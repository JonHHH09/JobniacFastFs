//! The opt-in Arbeitnow tools over the MCP wire.
//!
//! Registration, published schemas and annotations, argument decoding (null versus absent versus
//! empty list, strict booleans, integers forwarded to the service), the scan and import envelopes
//! through fake ports, the error envelopes, the Java instant text at the output boundary and the
//! no-echo rule for the candidate token. The 14 instant cases are core tests (`instant.rs`); the
//! output boundary is checked here. All data is synthetic.

mod common;

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use jobniac_core::arbeitnow::import::{ImportOutcome, ImportRequest, ImportStatus};
use jobniac_core::arbeitnow::instant::ArbeitnowInstant;
use jobniac_core::arbeitnow::scan::{ScanRequest, ScanResult, ScannedJob};
use jobniac_core::errors::{ApplicationError, ErrorCode, validation_error};
use jobniac_core::jobs::{JobAggregate, JobRecord};
use jobniac_mcp::tools::arbeitnow::{ArbeitnowImportPort, ArbeitnowPorts, ArbeitnowScanPort};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use serde_json::{Map, Value, json};
use time::macros::datetime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;

const SCAN: &str = "scan_arbeitnow_jobs";
const IMPORT: &str = "import_arbeitnow_job";
const NOTICE: &str = "Upstream results are mutable; this scan is not a snapshot.";
const SECRET_TOKEN: &str = "secret-payload.secret-signature";

/// A scan port that records its requests and answers with a fixed outcome.
#[derive(Debug)]
struct FakeScan {
    outcome: Result<ScanResult, ApplicationError>,
    requests: Mutex<Vec<ScanRequest>>,
}

impl FakeScan {
    fn new(outcome: Result<ScanResult, ApplicationError>) -> Arc<Self> {
        Arc::new(Self {
            outcome,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ScanRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl ArbeitnowScanPort for FakeScan {
    async fn scan(&self, request: ScanRequest) -> Result<ScanResult, ApplicationError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
        self.outcome.clone()
    }
}

/// An import port that records its requests and answers with a fixed outcome.
#[derive(Debug)]
struct FakeImport {
    outcome: Result<ImportOutcome, ApplicationError>,
    requests: Mutex<Vec<ImportRequest>>,
}

impl FakeImport {
    fn new(outcome: Result<ImportOutcome, ApplicationError>) -> Arc<Self> {
        Arc::new(Self {
            outcome,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ImportRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl ArbeitnowImportPort for FakeImport {
    async fn import(&self, request: ImportRequest) -> Result<ImportOutcome, ApplicationError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
        self.outcome.clone()
    }
}

fn instant(second: i64, nanosecond: u32) -> Result<ArbeitnowInstant, Box<dyn Error>> {
    Ok(ArbeitnowInstant::new(second, nanosecond)?)
}

fn scanned_job(slug: &str, posted_at: Option<ArbeitnowInstant>) -> ScannedJob {
    ScannedJob {
        slug: slug.to_owned(),
        canonical_url: format!("https://arbeitnow.com/view/{slug}"),
        company: "Example GmbH".to_owned(),
        title: "Rust Engineer".to_owned(),
        location: "Berlin".to_owned(),
        remote: posted_at.is_some(),
        posted_at,
        tags: if posted_at.is_some() {
            vec!["Rust".to_owned(), "SQL".to_owned()]
        } else {
            Vec::new()
        },
        job_types: if posted_at.is_some() {
            vec!["Full Time".to_owned()]
        } else {
            Vec::new()
        },
        description_excerpt: "Description".to_owned(),
        candidate_token: format!("{slug}.token"),
    }
}

fn scan_result(jobs: Vec<ScannedJob>, next_cursor: Option<&str>) -> ScanResult {
    ScanResult {
        source: "arbeitnow",
        source_docs_url: "https://www.arbeitnow.com/api/job-board-api",
        jobs,
        pages_scanned: 2,
        records_inspected: 7,
        next_cursor: next_cursor.map(str::to_owned),
        consistency_notice: NOTICE,
    }
}

fn aggregate() -> JobAggregate {
    JobAggregate {
        job: JobRecord {
            id: Uuid::from_u128(0xA1),
            source_method: "link".to_owned(),
            source_label: Some("arbeitnow".to_owned()),
            title: "Rust Engineer".to_owned(),
            company: Some("Example GmbH".to_owned()),
            location: Some("Berlin".to_owned()),
            description: "Description".to_owned(),
            experience_requirement: None,
            employment_type: None,
            seniority: None,
            posted_at: None,
            canonical_fingerprint: "f".repeat(64),
            created_at: datetime!(2026-10-09 12:00 UTC),
            updated_at: datetime!(2026-10-09 12:00 UTC),
            revision: 0,
        },
        skills: Vec::new(),
        link_ingestion: None,
        text_ingestion: None,
    }
}

fn services(
    scan: Option<Arc<FakeScan>>,
    import: Option<Arc<FakeImport>>,
) -> jobniac_mcp::McpServices {
    let mut services = common::services();
    services.arbeitnow = ArbeitnowPorts {
        scan: scan.map(|port| port as Arc<dyn ArbeitnowScanPort>),
        import: import.map(|port| port as Arc<dyn ArbeitnowImportPort>),
    };
    services
}

async fn call(
    services: jobniac_mcp::McpServices,
    name: &'static str,
    arguments: Value,
) -> Result<CallToolResult, Box<dyn Error>> {
    let client = common::connect(services).await?;
    let map: Map<String, Value> = match arguments {
        Value::Object(map) => map,
        _ => return Err("arguments must be an object".into()),
    };
    Ok(client
        .call_tool(CallToolRequestParams::new(name).with_arguments(map))
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

async fn listed(
    services: jobniac_mcp::McpServices,
) -> Result<Vec<rmcp::model::Tool>, Box<dyn Error>> {
    let client = common::connect(services).await?;
    Ok(client.list_all_tools().await?)
}

#[tokio::test]
async fn tools_are_absent_without_injection_and_registered_independently() -> TestResult {
    let names = |tools: Vec<rmcp::model::Tool>| -> Vec<String> {
        tools
            .into_iter()
            .map(|tool| tool.name.to_string())
            .filter(|name| name == SCAN || name == IMPORT)
            .collect()
    };
    assert!(names(listed(services(None, None)).await?).is_empty());
    let scan = FakeScan::new(Ok(scan_result(Vec::new(), None)));
    assert_eq!(names(listed(services(Some(scan), None)).await?), [SCAN]);
    let import = FakeImport::new(Err(ApplicationError::of(ErrorCode::InternalError)));
    assert_eq!(names(listed(services(None, Some(import))).await?), [IMPORT]);
    Ok(())
}

#[tokio::test]
async fn descriptions_annotations_and_schemas_are_the_pinned_ones() -> TestResult {
    let scan = FakeScan::new(Ok(scan_result(Vec::new(), None)));
    let import = FakeImport::new(Err(ApplicationError::of(ErrorCode::InternalError)));
    let tools = listed(services(Some(scan), Some(import))).await?;
    let find = |name: &str| tools.iter().find(|tool| tool.name == name);
    let scan_tool = find(SCAN).ok_or("scan tool must be listed")?;
    let import_tool = find(IMPORT).ok_or("import tool must be listed")?;

    assert_eq!(
        scan_tool.description.as_deref(),
        Some(
            "Read-only scan of the Arbeitnow public job-board API. \
             Results are mutable and are not a snapshot."
        )
    );
    let hints = |tool: &rmcp::model::Tool| {
        let a = tool.annotations.clone();
        (
            a.as_ref().and_then(|a| a.read_only_hint),
            a.as_ref().and_then(|a| a.destructive_hint),
            a.as_ref().and_then(|a| a.idempotent_hint),
            a.as_ref().and_then(|a| a.open_world_hint),
        )
    };
    assert_eq!(
        hints(scan_tool),
        (Some(true), Some(false), Some(false), Some(true))
    );
    assert_eq!(
        import_tool.description.as_deref(),
        Some("Import one signed, short-lived Arbeitnow scan candidate without refetching it.")
    );
    assert_eq!(
        hints(import_tool),
        (Some(false), Some(false), Some(true), Some(false))
    );

    let scan_schema = Value::Object(scan_tool.input_schema.as_ref().clone());
    assert_eq!(scan_schema["properties"].as_object().map(Map::len), Some(1));
    assert!(scan_schema.get("required").is_none());
    let mut keys: Vec<&str> = scan_schema["properties"]["request"]["properties"]
        .as_object()
        .ok_or("request schema has no properties")?
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "cursor",
            "jobTypes",
            "limit",
            "location",
            "maxPages",
            "query",
            "remoteOnly",
            "tags",
            "visaSponsorship"
        ]
    );
    let import_schema = Value::Object(import_tool.input_schema.as_ref().clone());
    assert_eq!(import_schema["required"], json!(["request"]));
    assert_eq!(
        import_schema["properties"]["request"]["required"],
        json!(["candidateToken"])
    );
    Ok(())
}

#[tokio::test]
async fn scan_envelope_is_exact_camel_case_with_nulls_and_the_request_is_decoded() -> TestResult {
    let posted = scanned_job("test-job", Some(instant(1_760_000_000, 0)?));
    let unposted = scanned_job("second", None);
    let port = FakeScan::new(Ok(scan_result(vec![posted, unposted], Some("cursor-2"))));
    let result = call(
        services(Some(port.clone()), None),
        SCAN,
        json!({"request": {
            "query": "rust", "location": "Berlin", "remoteOnly": true, "visaSponsorship": false,
            "tags": ["Rust"], "jobTypes": ["Full Time"], "limit": 2, "maxPages": 3,
            "cursor": "cursor-1"
        }}),
    )
    .await?;

    assert_eq!(result.is_error, Some(false));
    let expected = json!({
        "source": "arbeitnow",
        "sourceDocsUrl": "https://www.arbeitnow.com/api/job-board-api",
        "jobs": [
            {
                "slug": "test-job",
                "canonicalUrl": "https://arbeitnow.com/view/test-job",
                "company": "Example GmbH",
                "title": "Rust Engineer",
                "location": "Berlin",
                "remote": true,
                "postedAt": "2025-10-09T08:53:20Z",
                "tags": ["Rust", "SQL"],
                "jobTypes": ["Full Time"],
                "descriptionExcerpt": "Description",
                "candidateToken": "test-job.token"
            },
            {
                "slug": "second",
                "canonicalUrl": "https://arbeitnow.com/view/second",
                "company": "Example GmbH",
                "title": "Rust Engineer",
                "location": "Berlin",
                "remote": false,
                "postedAt": null,
                "tags": [],
                "jobTypes": [],
                "descriptionExcerpt": "Description",
                "candidateToken": "second.token"
            }
        ],
        "pagesScanned": 2,
        "recordsInspected": 7,
        "nextCursor": "cursor-2",
        "consistencyNotice": NOTICE
    });
    assert_eq!(result.structured_content, Some(expected.clone()));
    assert_eq!(serde_json::from_str::<Value>(&text(&result))?, expected);
    assert_eq!(
        port.requests(),
        vec![ScanRequest {
            query: Some("rust".to_owned()),
            location: Some("Berlin".to_owned()),
            remote_only: Some(true),
            visa_sponsorship: Some(false),
            tags: Some(vec!["Rust".to_owned()]),
            job_types: Some(vec!["Full Time".to_owned()]),
            limit: Some(2),
            max_pages: Some(3),
            cursor: Some("cursor-1".to_owned()),
        }]
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_null_or_empty_scan_request_maps_to_the_default_request() -> TestResult {
    for arguments in [json!({}), json!({"request": null}), json!({"request": {}})] {
        let port = FakeScan::new(Ok(scan_result(Vec::new(), None)));
        let result = call(services(Some(port.clone()), None), SCAN, arguments).await?;
        assert_eq!(result.is_error, Some(false));
        let value = result.structured_content.ok_or("structured content")?;
        assert_eq!(value["jobs"], json!([]));
        assert_eq!(value["nextCursor"], Value::Null);
        assert_eq!(port.requests(), vec![ScanRequest::default()]);
    }
    Ok(())
}

#[tokio::test]
async fn null_lists_stay_none_and_empty_lists_stay_empty() -> TestResult {
    let port = FakeScan::new(Ok(scan_result(Vec::new(), None)));
    call(
        services(Some(port.clone()), None),
        SCAN,
        json!({"request": {"tags": null, "jobTypes": [], "query": null}}),
    )
    .await?;
    let requests = port.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].tags, None);
    assert_eq!(requests[0].job_types, Some(Vec::new()));
    assert_eq!(requests[0].query, None);
    Ok(())
}

#[tokio::test]
async fn strict_types_are_enforced_and_integers_are_forwarded_to_the_service() -> TestResult {
    for (body, location, kind) in [
        (
            json!({"remoteOnly": "true"}),
            "request.remoteOnly",
            "bool_type",
        ),
        (
            json!({"visaSponsorship": 1}),
            "request.visaSponsorship",
            "bool_type",
        ),
        (json!({"limit": "5"}), "request.limit", "int_type"),
        (json!({"maxPages": 1.5}), "request.maxPages", "int_type"),
        (json!({"query": 5}), "request.query", "string_type"),
        (json!({"tags": "Rust"}), "request.tags", "list_type"),
        (json!({"tags": ["a", 2]}), "request.tags.1", "string_type"),
    ] {
        let port = FakeScan::new(Ok(scan_result(Vec::new(), None)));
        let result = call(
            services(Some(port.clone()), None),
            SCAN,
            json!({ "request": body }),
        )
        .await?;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, None);
        let rendered = text(&result);
        assert!(rendered.contains(location), "{rendered}");
        assert!(rendered.contains(&format!("[type={kind}]")), "{rendered}");
        assert!(port.requests().is_empty());
    }
    // `Int32` is a schema hint only: the adapter forwards the integer and the service decides.
    let port = FakeScan::new(Ok(scan_result(Vec::new(), None)));
    let result = call(
        services(Some(port.clone()), None),
        SCAN,
        json!({"request": {"limit": 2_147_483_648_i64}}),
    )
    .await?;
    assert_eq!(result.is_error, Some(false));
    assert_eq!(port.requests()[0].limit, Some(2_147_483_648));
    Ok(())
}

#[tokio::test]
async fn posted_at_uses_java_instant_text_below_year_zero_and_above_9999() -> TestResult {
    let cases = [
        (0, 500_000_000, "1970-01-01T00:00:00.500Z"),
        (253_402_300_800, 0, "+10000-01-01T00:00:00Z"),
        (-62_167_219_201, 0, "-0001-12-31T23:59:59Z"),
        (
            31_556_889_864_403_199,
            999_999_999,
            "+1000000000-12-31T23:59:59.999999999Z",
        ),
        (-31_557_014_167_219_200, 0, "-1000000000-01-01T00:00:00Z"),
    ];
    for (second, nanosecond, expected) in cases {
        let job = scanned_job("edge", Some(instant(second, nanosecond)?));
        let port = FakeScan::new(Ok(scan_result(vec![job], None)));
        let result = call(services(Some(port), None), SCAN, json!({})).await?;
        let value = result.structured_content.ok_or("structured content")?;
        assert_eq!(value["jobs"][0]["postedAt"], json!(expected));
    }
    Ok(())
}

#[tokio::test]
async fn import_passes_the_request_and_returns_the_status_and_unchanged_aggregate() -> TestResult {
    let outcome = ImportOutcome {
        status: ImportStatus::CreatedJob,
        job: aggregate(),
    };
    let port = FakeImport::new(Ok(outcome.clone()));
    let result = call(
        services(None, Some(port.clone())),
        IMPORT,
        json!({"request": {"candidateToken": "signed-token"}}),
    )
    .await?;
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result.structured_content,
        Some(json!({"status": "created_job", "job": serde_json::to_value(&outcome.job)?}))
    );
    assert_eq!(
        port.requests(),
        vec![ImportRequest {
            candidate_token: Some("signed-token".to_owned())
        }]
    );

    let reused = FakeImport::new(Ok(ImportOutcome {
        status: ImportStatus::ReusedExistingJob,
        job: aggregate(),
    }));
    let result = call(
        services(None, Some(reused)),
        IMPORT,
        json!({"request": {"candidateToken": "signed-token"}}),
    )
    .await?;
    let value = result.structured_content.ok_or("structured content")?;
    assert_eq!(value["status"], json!("reused_existing_job"));
    Ok(())
}

#[tokio::test]
async fn a_missing_blank_or_null_token_is_refused_before_the_service_with_empty_details()
-> TestResult {
    let expected = json!({
        "code": "validation_error",
        "message": "candidateToken is required",
        "details": {}
    });
    for token in [json!(null), json!(""), json!("   "), json!("\u{2003}\t")] {
        let port = FakeImport::new(Err(ApplicationError::of(ErrorCode::InternalError)));
        let result = call(
            services(None, Some(port.clone())),
            IMPORT,
            json!({"request": {"candidateToken": token}}),
        )
        .await?;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, Some(expected.clone()));
        assert_eq!(
            text(&result),
            r#"{"code":"validation_error","message":"candidateToken is required","details":{}}"#
        );
        assert!(port.requests().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn a_missing_request_or_token_key_is_an_argument_rejection_without_an_envelope() -> TestResult
{
    for arguments in [json!({}), json!({"request": null}), json!({"request": {}})] {
        let port = FakeImport::new(Err(ApplicationError::of(ErrorCode::InternalError)));
        let result = call(services(None, Some(port.clone())), IMPORT, arguments).await?;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content, None);
        assert!(port.requests().is_empty());
    }
    let port = FakeImport::new(Err(ApplicationError::of(ErrorCode::InternalError)));
    let result = call(
        services(None, Some(port.clone())),
        IMPORT,
        json!({"request": {"candidateToken": 5}}),
    )
    .await?;
    assert_eq!(result.structured_content, None);
    assert!(text(&result).contains("[type=string_type]"));
    assert!(port.requests().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_forged_candidate_error_is_byte_equal_and_never_echoes_the_token() -> TestResult {
    let port = FakeImport::new(Err(validation_error(
        "Invalid Arbeitnow candidate token",
        None,
        None,
    )));
    let result = call(
        services(None, Some(port)),
        IMPORT,
        json!({"request": {"candidateToken": SECRET_TOKEN}}),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content,
        Some(json!({
            "code": "validation_error",
            "message": "Invalid Arbeitnow candidate token",
            "details": {}
        }))
    );
    assert_eq!(
        text(&result),
        r#"{"code":"validation_error","message":"Invalid Arbeitnow candidate token","details":{}}"#
    );
    let wire = format!("{}{}", text(&result), serde_json::to_string(&result)?);
    assert!(!wire.contains("secret-"));
    Ok(())
}

#[tokio::test]
async fn application_errors_are_forwarded_verbatim_for_both_tools() -> TestResult {
    let error = validation_error("Invalid Arbeitnow scan request", None, None);
    let port = FakeScan::new(Err(error.clone()));
    let result = call(
        services(Some(port), None),
        SCAN,
        json!({"request": {"query": "rust"}}),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content,
        Some(serde_json::to_value(error.to_envelope())?)
    );

    let board = ApplicationError::new(
        ErrorCode::UpstreamRateLimited,
        Some("Arbeitnow request failed"),
        [("provider", "arbeitnow"), ("retryAfterSeconds", "30")],
    );
    let port = FakeImport::new(Err(board.clone()));
    let result = call(
        services(None, Some(port)),
        IMPORT,
        json!({"request": {"candidateToken": "token"}}),
    )
    .await?;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content,
        Some(serde_json::to_value(board.to_envelope())?)
    );
    Ok(())
}
