//! Behaviour of the profile use cases: the profile aggregate, search, the profile list and personal
//! details. The repositories are the in-memory doubles of `profile_support`.

mod profile_support;

use std::sync::Arc;

use jobniac_core::clock::{Clock, FixedClock, parse_instant};
use jobniac_core::errors::{AppError, error_envelope};
use jobniac_core::pagination::{CursorCodec, CursorKey, Page, SearchCandidates};
use jobniac_core::profiles::personal_details::PersonalDetailsEnvelope;
use jobniac_core::profiles::personal_details::PersonalDetailsRecord;
use jobniac_core::profiles::search::ProfileSearchCandidate;
use jobniac_core::profiles::{
    PersonalDetailsService, ProfileGraph, ProfileIdentity, ProfileListService,
    ProfileSearchService, ProfileService, profile_search_terms,
};
use serde::Serialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use profile_support::{
    MemoryList, MemoryPersonal, MemoryProfiles, MemorySearch, MemoryUow, SequenceClock,
};

const CONTACT_ID: &str = "11111111-1111-1111-1111-111111111111";
const LINK_ID: &str = "22222222-2222-2222-2222-222222222222";
const SKILL_ID: &str = "33333333-3333-3333-3333-333333333333";
const LANGUAGE_ID: &str = "44444444-4444-4444-4444-444444444444";
const EDUCATION_ID: &str = "55555555-5555-5555-5555-555555555555";
const EXPERIENCE_ID: &str = "66666666-6666-6666-6666-666666666666";
const PROJECT_ID: &str = "77777777-7777-7777-7777-777777777777";
const TECHNOLOGY_ID: &str = "88888888-8888-8888-8888-888888888888";
const PERSONAL_ID: &str = "12345678-1234-1234-1234-123456789abc";
const NOW_TEXT: &str = "2026-09-28T12:30:15.123Z";
const LATER_TEXT: &str = "2026-09-29T10:00:00Z";

type Service = ProfileService<MemoryUow, MemoryProfiles>;

fn at(text: &str) -> OffsetDateTime {
    match parse_instant(text) {
        Ok(value) => value,
        Err(error) => panic!("bad instant {text}: {error}"),
    }
}

fn to_value<T: Serialize>(value: &T) -> Value {
    match serde_json::to_value(value) {
        Ok(json) => json,
        Err(error) => panic!("value does not serialize: {error}"),
    }
}

fn envelope(error: &AppError) -> Value {
    to_value(&error_envelope(error))
}

fn expect_err<T: std::fmt::Debug>(result: Result<T, AppError>, what: &str) -> AppError {
    match result {
        Err(error) => error,
        Ok(value) => panic!("{what} unexpectedly succeeded: {value:?}"),
    }
}

fn expect_ok<T>(result: Result<T, AppError>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what} failed: {error}"),
    }
}

fn service(repo: &MemoryProfiles, clock: Arc<dyn Clock>) -> Service {
    ProfileService::new(MemoryUow, repo.clone(), clock)
}

fn fixed(text: &str) -> Arc<dyn Clock> {
    Arc::new(FixedClock::new(at(text)))
}

fn profile_request() -> Value {
    json!({
        "fullName": " Ada Lovelace ",
        "email": " ADA@EXAMPLE.TEST ",
        "summary": " Analytical engine ",
        "contacts": [{
            "id": CONTACT_ID,
            "contactType": " Phone ",
            "contactValue": " 555-0100 ",
            "label": " mobile "
        }],
        "links": [{
            "id": LINK_ID,
            "linkType": " Portfolio ",
            "url": "HTTP://EXAMPLE.TEST/ada/?source=profile#bio",
            "label": " personal site "
        }],
        "skills": [{
            "id": SKILL_ID,
            "skill": " Python ",
            "normalizedSkill": null,
            "category": " Programming ",
            "displayOrder": 2
        }],
        "languages": [{
            "id": LANGUAGE_ID,
            "language": " English ",
            "normalizedLanguage": null,
            "proficiency": " Fluent ",
            "displayOrder": 1
        }],
        "education": [{
            "id": EDUCATION_ID,
            "institution": " University ",
            "degree": " BSc ",
            "field": " Mathematics ",
            "location": " London ",
            "startDate": "2018-01-01",
            "endDate": "2022-01-01",
            "relevantFocus": " Computing "
        }],
        "experiences": [{
            "id": EXPERIENCE_ID,
            "company": " Analytical Engines Ltd ",
            "title": " Programmer ",
            "location": " London ",
            "startDate": "2022-01-01",
            "endDate": null,
            "description": " Built a compiler ",
            "displayOrder": 3
        }],
        "projects": [{
            "id": PROJECT_ID,
            "name": " Notes ",
            "url": " https://example.test/notes ",
            "description": " Project notebook ",
            "displayOrder": 4,
            "technologies": [{
                "id": TECHNOLOGY_ID,
                "technology": " PostgreSQL ",
                "normalizedTechnology": null,
                "displayOrder": 5
            }]
        }]
    })
}

fn posting_weight(graph: &ProfileGraph, field_key: &str, term: &str) -> Option<i64> {
    profile_search_terms(graph)
        .iter()
        .find(|posting| posting.field_key == field_key && posting.term == term)
        .map(|posting| posting.weight)
}

fn validation_details(field: &str, reason: &str) -> Value {
    json!({
        "code": "validation_error",
        "message": "Invalid profile write request",
        "details": {"field": field, "reason": reason}
    })
}

/// The service futures must be `Send` for a concrete adapter (Tauri commands run them on a
/// multi-threaded runtime). Checked at compile time for each service over the in-memory doubles.
#[test]
fn service_futures_are_send_for_concrete_adapters() {
    fn assert_send<T: Send>(_: &T) {}
    let profiles = MemoryProfiles::new();
    let profile_service = service(&profiles, fixed(NOW_TEXT));
    assert_send(&profile_service.get_profile(PERSONAL_ID));
    assert_send(&profile_service.create_profile(&profile_request()));
    let personal = fresh_personal();
    let personal_svc = personal_service(&personal, fixed(NOW_TEXT));
    assert_send(&personal_svc.get_personal_details(PERSONAL_ID));
    assert_send(&personal_svc.update_personal_details(PERSONAL_ID, &json!(0), &null_request()));
    let search = search_service(expect_ok(
        SearchCandidates::new(0, false, Vec::new()),
        "empty",
    ));
    assert_send(&search.search_profiles(&json!({"query": "java"})));
    let list = ProfileListService::new(
        MemoryUow,
        MemoryList::new(Page {
            items: Vec::new(),
            next_cursor: None,
        }),
        codec(),
    );
    assert_send(&list.list_profiles(None, None));
}

// ---- profile aggregate ----

#[tokio::test]
async fn create_returns_normalized_graph_and_search_postings() {
    let repo = MemoryProfiles::new();
    let graph = expect_ok(
        service(&repo, fixed(NOW_TEXT))
            .create_profile(&profile_request())
            .await,
        "create",
    );
    let value = to_value(&graph);
    let mut keys: Vec<&str> = value
        .as_object()
        .map(|map| map.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "contacts",
            "education",
            "experiences",
            "languages",
            "links",
            "profile",
            "projectTechnologies",
            "projects",
            "skills"
        ]
    );
    let profile_id = graph.profile.id.to_string();
    assert_eq!(
        value["profile"],
        json!({
            "id": profile_id,
            "fullName": "Ada Lovelace",
            "email": "ada@example.test",
            "summary": "Analytical engine",
            "rawResumeText": null,
            "createdAt": "2026-09-28T12:30:15.123Z",
            "updatedAt": "2026-09-28T12:30:15.123Z",
            "embedding": null,
            "revision": 0
        })
    );
    assert_eq!(
        value["contacts"][0],
        json!({
            "id": CONTACT_ID,
            "profileId": profile_id,
            "contactType": "phone",
            "contactValue": "555-0100",
            "label": "mobile",
            "createdAt": "2026-09-28T12:30:15.123Z",
            "updatedAt": "2026-09-28T12:30:15.123Z"
        })
    );
    assert_eq!(value["links"][0]["url"], json!("https://example.test/ada"));
    assert_eq!(value["skills"][0]["normalizedSkill"], json!("python"));
    assert_eq!(
        value["languages"][0]["normalizedLanguage"],
        json!("english")
    );
    assert_eq!(value["education"][0]["startDate"], json!("2018-01-01"));
    assert_eq!(
        value["experiences"][0]["description"],
        json!("Built a compiler")
    );
    assert_eq!(value["projects"][0]["id"], json!(PROJECT_ID));
    assert_eq!(
        value["projects"][0]["technologies"][0]["id"],
        json!(TECHNOLOGY_ID)
    );
    assert_eq!(
        value["projectTechnologies"],
        value["projects"][0]["technologies"]
    );
    assert_eq!(posting_weight(&graph, "profile.fullName", "ada"), Some(8));
    assert_eq!(
        posting_weight(&graph, &format!("skills:{SKILL_ID}:normalized"), "python"),
        Some(7)
    );
    assert_eq!(
        posting_weight(
            &graph,
            &format!("technologies:{TECHNOLOGY_ID}:technology"),
            "postgresql"
        ),
        Some(5)
    );
    assert_eq!(repo.creates(), 1);
    assert_eq!(repo.stored(graph.profile.id), Some(graph.clone()));
}

#[tokio::test]
async fn get_returns_graph_and_reports_missing_profile() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let read = expect_ok(
        svc.get_profile(&created.profile.id.to_string()).await,
        "get",
    );
    assert_eq!(read, created);
    let missing = expect_err(
        svc.get_profile("99999999-9999-9999-9999-999999999999")
            .await,
        "missing get",
    );
    assert_eq!(
        envelope(&missing),
        json!({
            "code": "not_found",
            "message": "Profile not found: 99999999-9999-9999-9999-999999999999",
            "details": {"resource": "profile", "profileId": "99999999-9999-9999-9999-999999999999"}
        })
    );
}

#[tokio::test]
async fn replace_advances_revision_keeps_creation_time_and_replaces_children() {
    let repo = MemoryProfiles::new();
    let clock = Arc::new(SequenceClock::new(vec![at(NOW_TEXT), at(LATER_TEXT)]));
    let svc = ProfileService::new(MemoryUow, repo.clone(), clock);
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = created.profile.id.to_string();
    let replacement =
        json!({"fullName": "Ada Byron", "email": "ADA@EXAMPLE.TEST", "summary": "Updated"});

    let updated = expect_ok(
        svc.update_profile(&profile_id, &json!(0), &replacement)
            .await,
        "update",
    );
    let value = to_value(&updated);
    assert_eq!(value["profile"]["id"], json!(profile_id));
    assert_eq!(value["profile"]["fullName"], json!("Ada Byron"));
    assert_eq!(
        value["profile"]["createdAt"],
        to_value(&created)["profile"]["createdAt"]
    );
    assert_eq!(value["profile"]["updatedAt"], json!(LATER_TEXT));
    assert_eq!(value["profile"]["revision"], json!(1));
    assert_eq!(value["contacts"], json!([]));
    assert_eq!(value["skills"], json!([]));
    assert_eq!(value["projects"], json!([]));
    assert_eq!(value["projectTechnologies"], json!([]));
    assert_eq!(repo.replaces(), 1);

    let stale = expect_err(
        svc.update_profile(&profile_id, &json!(0), &replacement)
            .await,
        "stale update",
    );
    assert_eq!(
        envelope(&stale),
        json!({
            "code": "conflict",
            "message": "Profile revision conflict",
            "details": {"resource": "profile", "profileId": profile_id, "expectedRevision": "0"}
        })
    );
    assert_eq!(
        expect_ok(svc.get_profile(&profile_id).await, "get"),
        updated
    );
}

#[tokio::test]
async fn project_patch_keeps_omitted_fields_and_replaces_only_supplied_technologies() {
    let repo = MemoryProfiles::new();
    let clock = Arc::new(SequenceClock::new(vec![
        at(NOW_TEXT),
        at(LATER_TEXT),
        at(LATER_TEXT),
        at(LATER_TEXT),
    ]));
    let svc = ProfileService::new(MemoryUow, repo.clone(), clock);
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = created.profile.id.to_string();
    let project = to_value(&created)["projects"][0].clone();

    let renamed = expect_ok(
        svc.update_profile_project(&json!({
            "profileId": profile_id,
            "projectId": PROJECT_ID,
            "expectedRevision": 0,
            "name": "  Research Notes  "
        }))
        .await,
        "rename",
    );
    let renamed = to_value(&renamed);
    assert_eq!(renamed["profileRevision"], json!(1));
    assert_eq!(renamed["project"]["name"], json!("Research Notes"));
    assert_eq!(renamed["project"]["url"], project["url"]);
    assert_eq!(renamed["project"]["description"], project["description"]);
    assert_eq!(renamed["project"]["technologies"], project["technologies"]);

    let cleared = expect_ok(
        svc.update_profile_project(&json!({
            "profileId": profile_id,
            "projectId": PROJECT_ID,
            "expectedRevision": 1,
            "technologies": []
        }))
        .await,
        "clear",
    );
    let cleared = to_value(&cleared);
    assert_eq!(cleared["profileRevision"], json!(2));
    assert_eq!(cleared["project"]["technologies"], json!([]));
    assert_eq!(
        to_value(&expect_ok(svc.get_profile(&profile_id).await, "get"))["projectTechnologies"],
        json!([])
    );

    let replaced = expect_ok(
        svc.update_profile_project(&json!({
            "profileId": profile_id,
            "projectId": PROJECT_ID,
            "expectedRevision": 2,
            "technologies": [{"technology": " SQLite ", "normalizedTechnology": null, "displayOrder": 1}]
        }))
        .await,
        "replace",
    );
    let replaced = to_value(&replaced);
    assert_eq!(replaced["profileRevision"], json!(3));
    assert_eq!(
        replaced["project"]["technologies"][0]["technology"],
        json!("SQLite")
    );
    assert_eq!(
        replaced["project"]["technologies"][0]["normalizedTechnology"],
        json!("sqlite")
    );
    assert_ne!(
        replaced["project"]["technologies"][0]["id"],
        json!(TECHNOLOGY_ID)
    );
    assert_eq!(
        to_value(&expect_ok(svc.get_profile(&profile_id).await, "get"))["projectTechnologies"],
        replaced["project"]["technologies"]
    );
    assert_eq!(repo.project_updates(), 3);
}

#[tokio::test]
async fn project_patch_never_persists_a_supplied_technology_identifier() {
    let repo = MemoryProfiles::new();
    let clock = Arc::new(SequenceClock::new(vec![
        at(NOW_TEXT),
        at(LATER_TEXT),
        at(LATER_TEXT),
    ]));
    let svc = ProfileService::new(MemoryUow, repo.clone(), clock);
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = created.profile.id.to_string();

    let patched = expect_ok(
        svc.update_profile_project(&json!({
            "profileId": profile_id,
            "projectId": PROJECT_ID,
            "expectedRevision": 0,
            "technologies": [{
                "id": TECHNOLOGY_ID,
                "technology": "SQLite",
                "displayOrder": 0
            }]
        }))
        .await,
        "patch with supplied technology id",
    );
    let patched = to_value(&patched);
    let returned_id = patched["project"]["technologies"][0]["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert_ne!(
        returned_id, TECHNOLOGY_ID,
        "the supplied id must not be returned"
    );
    assert!(
        Uuid::parse_str(&returned_id).is_ok(),
        "the fresh id is a UUID"
    );

    let stored = to_value(&expect_ok(svc.get_profile(&profile_id).await, "get"));
    assert_eq!(stored["projectTechnologies"][0]["id"], json!(returned_id));
    assert!(
        !stored.to_string().contains(TECHNOLOGY_ID),
        "the supplied technology id is not persisted anywhere in the graph"
    );
}

#[tokio::test]
async fn invalid_create_fails_before_any_write() {
    let repo = MemoryProfiles::new();
    let error = expect_err(
        service(&repo, fixed(NOW_TEXT))
            .create_profile(&json!({"fullName": "Ada", "email": "not-an-email"}))
            .await,
        "invalid create",
    );
    assert_eq!(
        envelope(&error),
        validation_details("email", "must be a valid email address")
    );
    assert_eq!(repo.creates(), 0);
}

#[tokio::test]
async fn project_patch_rejects_blank_supplied_normalized_technology() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let error = expect_err(
        svc.update_profile_project(&json!({
            "profileId": created.profile.id.to_string(),
            "projectId": PROJECT_ID,
            "expectedRevision": 0,
            "technologies": [{"technology": "SQLite", "normalizedTechnology": " ", "displayOrder": 0}]
        }))
        .await,
        "blank normalized",
    );
    assert_eq!(
        envelope(&error)["details"],
        json!({"field": "technologies[0].normalizedTechnology", "reason": "must not be blank when supplied"})
    );
    assert_eq!(repo.project_updates(), 0);
}

#[tokio::test]
async fn display_order_overflow_is_validation_before_persistence() {
    for collection in ["skills", "languages", "experiences", "projects"] {
        let repo = MemoryProfiles::new();
        let mut request = profile_request();
        request[collection][0]["displayOrder"] = json!(2_147_483_648_u64);
        let error = expect_err(
            service(&repo, fixed(NOW_TEXT))
                .create_profile(&request)
                .await,
            collection,
        );
        assert_eq!(
            envelope(&error)["details"]["field"],
            json!(format!("{collection}[0].displayOrder")),
            "{collection}"
        );
        assert_eq!(repo.creates(), 0, "{collection}");
    }
}

#[tokio::test]
async fn project_technology_display_order_overflow_is_validation_before_persistence() {
    let repo = MemoryProfiles::new();
    let mut request = profile_request();
    request["projects"][0]["technologies"][0]["displayOrder"] = json!(2_147_483_648_u64);
    let error = expect_err(
        service(&repo, fixed(NOW_TEXT))
            .create_profile(&request)
            .await,
        "overflow",
    );
    assert_eq!(
        envelope(&error)["details"]["field"],
        json!("projects[0].technologies[0].displayOrder")
    );
    assert_eq!(repo.creates(), 0);
}

#[tokio::test]
async fn display_order_accepts_postgresql_integer_boundaries() {
    for value in [0_u64, 2_147_483_647] {
        let repo = MemoryProfiles::new();
        let mut request = profile_request();
        request["skills"][0]["displayOrder"] = json!(value);
        let graph = expect_ok(
            service(&repo, fixed(NOW_TEXT))
                .create_profile(&request)
                .await,
            "create",
        );
        assert_eq!(
            graph.skills[0].display_order,
            i64::try_from(value).unwrap_or_default()
        );
    }
}

#[tokio::test]
async fn project_patch_rejects_display_order_overflow_without_writing() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let original = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = original.profile.id.to_string();
    for changes in [
        json!({"displayOrder": 2_147_483_648_u64}),
        json!({"technologies": [{"technology": "Rust", "displayOrder": 2_147_483_648_u64}]}),
    ] {
        let mut request =
            json!({"profileId": profile_id, "projectId": PROJECT_ID, "expectedRevision": 0});
        if let (Some(target), Some(extra)) = (request.as_object_mut(), changes.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        let error = expect_err(svc.update_profile_project(&request).await, "overflow patch");
        assert_eq!(envelope(&error)["code"], json!("validation_error"));
    }
    assert_eq!(repo.project_updates(), 0);
    assert_eq!(
        expect_ok(svc.get_profile(&profile_id).await, "get"),
        original
    );
}

#[tokio::test]
async fn uuid_text_is_admitted_in_the_canonical_form_only() {
    let repo = MemoryProfiles::new();
    let mut request = profile_request();
    request["contacts"][0]["id"] = json!("{11111111-1111-1111-1111-111111111111}");
    let error = expect_err(
        service(&repo, fixed(NOW_TEXT))
            .create_profile(&request)
            .await,
        "braced id",
    );
    assert_eq!(
        envelope(&error),
        json!({
            "code": "validation_error",
            "message": "Invalid profile request",
            "details": {"field": "contacts[0].id", "reason": "must be a valid UUID"}
        })
    );
    request["contacts"][0]["id"] = json!("11111111111111111111111111111111");
    let error = expect_err(
        service(&repo, fixed(NOW_TEXT))
            .create_profile(&request)
            .await,
        "simple id",
    );
    assert_eq!(
        envelope(&error)["details"]["field"],
        json!("contacts[0].id")
    );
    assert_eq!(repo.creates(), 0);
}

// ---- search ----

fn identity(id: &str, name: &str) -> ProfileIdentity {
    ProfileIdentity::new(
        Uuid::parse_str(id).unwrap_or_else(|_| Uuid::nil()),
        name.to_owned(),
        "ada@example.test".to_owned(),
        Some("Java systems engineer".to_owned()),
        at("2026-07-13T12:00:00Z"),
        at("2026-07-14T12:00:00Z"),
        4,
    )
}

fn candidate(identity: ProfileIdentity, score: i64, keys: &[&str]) -> ProfileSearchCandidate {
    ProfileSearchCandidate {
        profile: identity,
        score,
        field_keys: keys.iter().map(|key| (*key).to_owned()).collect(),
    }
}

fn search_service(
    result: SearchCandidates<ProfileSearchCandidate>,
) -> ProfileSearchService<MemoryUow, MemorySearch> {
    ProfileSearchService::new(MemoryUow, MemorySearch::new(result))
}

#[tokio::test]
async fn search_returns_envelope_and_sanitizes_identity() {
    let result = expect_ok(
        SearchCandidates::new(
            1,
            false,
            vec![candidate(
                identity("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "Ada Lovelace"),
                21,
                &[
                    "experience:bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb:title",
                    "skills:cccccccc-cccc-cccc-cccc-cccccccccccc:skill",
                    "profile.fullName",
                ],
            )],
        ),
        "candidates",
    );
    let response = expect_ok(
        search_service(result)
            .search_profiles(&json!({"query": "  JAVA SPRIN  ", "limit": 5}))
            .await,
        "search",
    );
    assert_eq!(
        to_value(&response),
        json!({
            "query": "JAVA SPRIN",
            "queryTokens": ["java", "sprin"],
            "totalMatches": 1,
            "matchedCount": 1,
            "hasMore": false,
            "returnedCount": 1,
            "profiles": [{
                "profile": {
                    "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                    "fullName": "Ada Lovelace",
                    "email": "ada@example.test",
                    "summary": "Java systems engineer",
                    "rawResumeText": null,
                    "createdAt": "2026-07-13T12:00:00Z",
                    "updatedAt": "2026-07-14T12:00:00Z",
                    "embedding": null,
                    "revision": 4
                },
                "score": 21,
                "matchedFields": ["profile.fullName", "skills", "experience.title"]
            }]
        })
    );
}

#[tokio::test]
async fn search_orders_by_score_then_identifier_and_defaults_the_limit() {
    let result = expect_ok(
        SearchCandidates::new(
            3,
            false,
            vec![
                candidate(
                    identity("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", "B"),
                    8,
                    &[],
                ),
                candidate(
                    identity("cccccccc-cccc-cccc-cccc-cccccccccccc", "C"),
                    9,
                    &[],
                ),
                candidate(
                    identity("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "A"),
                    8,
                    &[],
                ),
            ],
        ),
        "candidates",
    );
    let response = expect_ok(
        search_service(result)
            .search_profiles(&json!({"query": "java"}))
            .await,
        "search",
    );
    assert_eq!(response.total_matches, Some(3));
    let order: Vec<String> = response
        .profiles
        .iter()
        .map(|m| m.profile.id.to_string())
        .collect();
    assert_eq!(
        order,
        [
            "cccccccc-cccc-cccc-cccc-cccccccccccc",
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        ]
    );
}

#[tokio::test]
async fn bounded_candidates_report_an_unknown_total() {
    let items = vec![
        candidate(
            identity("00000000-0000-0000-0000-000000000002", "Two"),
            1,
            &[],
        ),
        candidate(
            identity("00000000-0000-0000-0000-000000000001", "One"),
            1,
            &[],
        ),
    ];
    let result = expect_ok(SearchCandidates::new(500, true, items), "candidates");
    let response = expect_ok(
        search_service(result)
            .search_profiles(&json!({"query": "java", "limit": 1}))
            .await,
        "search",
    );
    let value = to_value(&response);
    assert_eq!(value["totalMatches"], json!(null));
    assert_eq!(value["matchedCount"], json!(500));
    assert_eq!(value["hasMore"], json!(true));
    assert_eq!(value["returnedCount"], json!(1));
}

#[tokio::test]
async fn search_validates_request_before_reading() {
    let cases = [
        (json!(null), "request", "must not be null"),
        (json!({"query": "  "}), "query", "must not be blank"),
        (
            json!({"query": "---"}),
            "query",
            "must contain searchable text",
        ),
        (
            json!({"query": "\u{1F600}".repeat(257)}),
            "query",
            "must not exceed 256 characters",
        ),
        (
            json!({"query": (0..17).map(|i| format!("term{i}")).collect::<Vec<_>>().join(" ")}),
            "query",
            "must contain at most 16 searchable terms",
        ),
        (
            json!({"query": "java", "limit": 0}),
            "limit",
            "must be between 1 and 100",
        ),
        (
            json!({"query": "java", "limit": 101}),
            "limit",
            "must be between 1 and 100",
        ),
        (
            json!({"query": "java", "limit": true}),
            "limit",
            "must be between 1 and 100",
        ),
    ];
    for (request, field, reason) in cases {
        let result = SearchCandidates::new(0, false, Vec::new());
        let repo = MemorySearch::new(expect_ok(result, "empty"));
        let svc = ProfileSearchService::new(MemoryUow, repo);
        let error = expect_err(svc.search_profiles(&request).await, &request.to_string());
        assert_eq!(
            envelope(&error),
            json!({
                "code": "validation_error",
                "message": "Invalid profile search request",
                "details": {"field": field, "reason": reason}
            })
        );
    }
}

#[tokio::test]
async fn search_accepts_exact_query_length_and_token_limits() {
    let empty = expect_ok(SearchCandidates::new(0, false, Vec::new()), "empty");
    let svc = search_service(empty);
    let longest = "x".repeat(256);
    let sixteen = (0..16)
        .map(|i| format!("term{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let first = expect_ok(
        svc.search_profiles(&json!({"query": longest.clone()}))
            .await,
        "longest",
    );
    let second = expect_ok(
        svc.search_profiles(&json!({"query": sixteen})).await,
        "sixteen",
    );
    assert_eq!(first.query_tokens, vec![longest]);
    assert_eq!(
        second.query_tokens,
        (0..16).map(|i| format!("term{i}")).collect::<Vec<_>>()
    );
}

fn identity_with_private_fields() -> ProfileIdentity {
    let mut value = identity("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "Ada Lovelace");
    value.raw_resume_text = Some("SECRET-RESUME-MARKER".to_owned());
    value.embedding = Some(vec![0.123_456]);
    value
}

#[tokio::test]
async fn search_forces_resume_text_and_embedding_to_null_whatever_the_repository_sent() {
    let result = expect_ok(
        SearchCandidates::new(
            1,
            false,
            vec![candidate(
                identity_with_private_fields(),
                3,
                &["profile.fullName"],
            )],
        ),
        "candidates",
    );
    let response = expect_ok(
        search_service(result)
            .search_profiles(&json!({"query": "java"}))
            .await,
        "search",
    );
    let text = to_value(&response).to_string();
    assert!(!text.contains("SECRET-RESUME-MARKER"));
    assert!(!text.contains("0.123456"));
    assert_eq!(response.profiles.len(), 1);
    assert_eq!(
        to_value(&response)["profiles"][0]["profile"]["rawResumeText"],
        Value::Null
    );
    assert_eq!(
        to_value(&response)["profiles"][0]["profile"]["embedding"],
        Value::Null
    );
}

#[tokio::test]
async fn list_profiles_forces_resume_text_and_embedding_to_null() {
    let repo = MemoryList::new(Page {
        items: vec![identity_with_private_fields()],
        next_cursor: None,
    });
    let service = ProfileListService::new(MemoryUow, repo, codec());
    let result = expect_ok(service.list_profiles(None, None).await, "list");
    let text = to_value(&result).to_string();
    assert!(!text.contains("SECRET-RESUME-MARKER"));
    assert!(!text.contains("0.123456"));
}

#[tokio::test]
async fn default_limit_returns_at_most_twenty_profiles() {
    let items: Vec<ProfileSearchCandidate> = (0..25)
        .map(|index| {
            candidate(
                identity(&format!("00000000-0000-0000-0000-{index:012x}"), "P"),
                1,
                &[],
            )
        })
        .collect();
    let result = expect_ok(SearchCandidates::new(25, false, items), "candidates");
    let response = expect_ok(
        search_service(result)
            .search_profiles(&json!({"query": "java"}))
            .await,
        "search",
    );
    assert_eq!(response.matched_count, 25);
    assert_eq!(response.returned_count, 20);
    assert_eq!(response.profiles.len(), 20);
}

// ---- profile list ----

fn codec() -> CursorCodec {
    let key = match CursorKey::from_secret("0123456789abcdef0123456789abcdef") {
        Ok(key) => key,
        Err(error) => panic!("test key refused: {error:?}"),
    };
    CursorCodec::new(key)
}

#[tokio::test]
async fn list_profiles_returns_the_object_envelope_and_profiles_scope() {
    let profile = identity("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "Ada Lovelace");
    let repo = MemoryList::new(Page {
        items: vec![profile.clone()],
        next_cursor: Some("opaque-next-cursor".to_owned()),
    });
    let service = ProfileListService::new(MemoryUow, repo.clone(), codec());
    let result = expect_ok(service.list_profiles(None, None).await, "list");
    let value = to_value(&result);
    assert_eq!(value["nextCursor"], json!("opaque-next-cursor"));
    assert_eq!(value["profiles"], to_value(&vec![profile]));
    let expected_fingerprint = expect_ok(
        codec().page_request(Some(20), None, "profiles", "all"),
        "page",
    )
    .fingerprint()
    .to_owned();
    assert_eq!(
        repo.requests(),
        vec![(20, "profiles".to_owned(), false, expected_fingerprint)]
    );
}

#[tokio::test]
async fn list_profiles_passes_a_valid_cursor_and_a_new_limit() {
    let created_at = at("2026-07-13T12:00:00Z");
    let snapshot_at = at("2026-07-14T12:00:00Z");
    let profile_id =
        Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap_or_else(|_| Uuid::nil());
    let cursor = expect_ok(
        expect_ok(
            codec().page_request(Some(1), None, "profiles", "all"),
            "page",
        )
        .next_cursor(snapshot_at, created_at, profile_id),
        "cursor",
    );
    let repo = MemoryList::new(Page {
        items: Vec::new(),
        next_cursor: None,
    });
    let service = ProfileListService::new(MemoryUow, repo.clone(), codec());
    let result = expect_ok(service.list_profiles(Some(7), Some(&cursor)).await, "list");
    assert_eq!(
        to_value(&result),
        json!({"profiles": [], "nextCursor": null})
    );
    let requests = repo.requests();
    assert_eq!(requests[0].0, 7);
    assert!(requests[0].2);
    assert_eq!(
        repo.last_position(),
        Some((snapshot_at, created_at, profile_id))
    );
}

// ---- personal details ----

fn personal_id() -> Uuid {
    Uuid::parse_str(PERSONAL_ID).unwrap_or_else(|_| Uuid::nil())
}

fn personal_service(
    repo: &MemoryPersonal,
    clock: Arc<dyn Clock>,
) -> PersonalDetailsService<MemoryUow, MemoryPersonal> {
    PersonalDetailsService::new(MemoryUow, repo.clone(), clock)
}

fn fresh_personal() -> MemoryPersonal {
    MemoryPersonal::with_profile(personal_id(), 0, at(NOW_TEXT))
}

fn null_request() -> Value {
    json!({"dateOfBirth": null, "nationality": null})
}

#[tokio::test]
async fn absent_optional_row_reads_without_a_write_and_uuid_is_canonical() {
    let repo = fresh_personal();
    let svc = personal_service(&repo, fixed(NOW_TEXT));
    let result = expect_ok(
        svc.get_personal_details(&PERSONAL_ID.to_uppercase()).await,
        "get",
    );
    assert_eq!(Some(result.clone()), repo.current());
    assert!(result.personal_details.is_none());
    assert_eq!(repo.writes(), 0);
}

#[tokio::test]
async fn first_write_then_explicit_clear() {
    let repo = fresh_personal();
    let svc = personal_service(&repo, fixed(NOW_TEXT));
    let result = expect_ok(
        svc.update_personal_details(
            PERSONAL_ID,
            &json!(0),
            &json!({"dateOfBirth": "2000-02-29", "nationality": "  Canada\u{2003}"}),
        )
        .await,
        "first write",
    );
    let value = to_value(&result);
    assert_eq!(value["profileRevision"], json!(1));
    assert_eq!(
        value["personalDetails"],
        json!({
            "profileId": PERSONAL_ID,
            "dateOfBirth": "2000-02-29",
            "nationality": "Canada",
            "photoDocumentId": null,
            "createdAt": "2026-09-28T12:30:15.123Z",
            "updatedAt": "2026-09-28T12:30:15.123Z"
        })
    );
    let cleared = expect_ok(
        svc.update_personal_details(PERSONAL_ID, &json!(1), &null_request())
            .await,
        "clear",
    );
    let value = to_value(&cleared);
    assert_eq!(value["profileRevision"], json!(2));
    assert_eq!(value["personalDetails"]["dateOfBirth"], json!(null));
    assert_eq!(value["personalDetails"]["nationality"], json!(null));
}

#[tokio::test]
async fn nationality_uses_java_strip_and_keeps_non_java_whitespace() {
    let cases: [(Option<&str>, Option<&str>); 6] = [
        (None, None),
        (Some(""), None),
        (Some(" \t\n\u{2003}"), None),
        (Some("\u{a0}"), Some("\u{a0}")),
        (Some("Canada"), Some("Canada")),
        (Some("\u{1F600}"), Some("\u{1F600}")),
    ];
    for (input, expected) in cases {
        let repo = fresh_personal();
        let nationality = input.map_or(Value::Null, |text| json!(text));
        let result = expect_ok(
            personal_service(&repo, fixed(NOW_TEXT))
                .update_personal_details(
                    PERSONAL_ID,
                    &json!(0),
                    &json!({"dateOfBirth": null, "nationality": nationality}),
                )
                .await,
            "nationality",
        );
        let details = result
            .personal_details
            .map(|details| details.nationality)
            .unwrap_or_default();
        assert_eq!(details.as_deref(), expected, "input {input:?}");
    }
}

#[tokio::test]
async fn canonical_dates_and_future_dates_are_accepted() {
    for birth in ["0001-01-01", "9999-12-31", "2024-02-29", "2099-01-01"] {
        let repo = fresh_personal();
        let result = expect_ok(
            personal_service(&repo, fixed(NOW_TEXT))
                .update_personal_details(
                    PERSONAL_ID,
                    &json!(0),
                    &json!({"dateOfBirth": birth, "nationality": null}),
                )
                .await,
            birth,
        );
        assert_eq!(
            to_value(&result)["personalDetails"]["dateOfBirth"],
            json!(birth)
        );
    }
}

#[tokio::test]
async fn invalid_requests_fail_before_any_io_without_private_values() {
    let invalid = [
        json!(null),
        json!([]),
        json!("private"),
        json!({}),
        json!({"dateOfBirth": null}),
        json!({"nationality": null}),
        json!({"dateOfBirth": null, "nationality": null, "photoDocumentId": "private"}),
        json!({"dateOfBirth": null, "nationality": null, "createdAt": "private"}),
        json!({"dateOfBirth": "", "nationality": null}),
        json!({"dateOfBirth": " ", "nationality": null}),
        json!({"dateOfBirth": "0000-01-01", "nationality": null}),
        json!({"dateOfBirth": "2024-02-30", "nationality": null}),
        json!({"dateOfBirth": "20240101", "nationality": null}),
        json!({"dateOfBirth": "\u{FF12}\u{FF10}\u{FF12}\u{FF14}-01-01", "nationality": null}),
        json!({"dateOfBirth": true, "nationality": null}),
        json!({"dateOfBirth": null, "nationality": true}),
        json!({"dateOfBirth": null, "nationality": "private\u{0}value"}),
    ];
    for request in invalid {
        let repo = fresh_personal();
        let error = expect_err(
            personal_service(&repo, fixed(NOW_TEXT))
                .update_personal_details(PERSONAL_ID, &json!(0), &request)
                .await,
            &request.to_string(),
        );
        assert_eq!(envelope(&error)["code"], json!("validation_error"));
        assert!(
            !envelope(&error).to_string().contains("private"),
            "{request}"
        );
        assert_eq!(repo.io_count(), 0, "{request}");
    }
}

#[tokio::test]
async fn invalid_revisions_are_refused_before_io() {
    let revisions = [
        json!(null),
        json!(true),
        json!(-1),
        json!(1.0),
        json!("0"),
        json!(9_223_372_036_854_775_808_u64),
    ];
    for revision in revisions {
        let repo = fresh_personal();
        let _ = expect_err(
            personal_service(&repo, fixed(NOW_TEXT))
                .update_personal_details(PERSONAL_ID, &revision, &null_request())
                .await,
            &revision.to_string(),
        );
        assert_eq!(repo.io_count(), 0, "{revision}");
    }
}

#[tokio::test]
async fn invalid_profile_identifier_does_not_echo_the_input() {
    let repo = fresh_personal();
    let error = expect_err(
        personal_service(&repo, fixed(NOW_TEXT))
            .get_personal_details("private-invalid")
            .await,
        "invalid id",
    );
    assert!(!envelope(&error).to_string().contains("private-invalid"));
}

#[tokio::test]
async fn missing_profile_is_not_the_absent_optional_row() {
    let repo = fresh_personal();
    repo.mark_missing();
    let error = expect_err(
        personal_service(&repo, fixed(NOW_TEXT))
            .get_personal_details(PERSONAL_ID)
            .await,
        "missing",
    );
    assert_eq!(
        envelope(&error),
        json!({
            "code": "not_found",
            "message": format!("Profile not found: {PERSONAL_ID}"),
            "details": {"resource": "profile", "profileId": PERSONAL_ID}
        })
    );
}

#[tokio::test]
async fn stale_and_maximum_revisions_conflict_without_writing() {
    for maximum in [false, true] {
        let revision = if maximum { i64::MAX } else { 1 };
        let expected = if maximum { i64::MAX } else { 0 };
        let repo = MemoryPersonal::with_profile(personal_id(), revision, at(NOW_TEXT));
        let error = expect_err(
            personal_service(&repo, fixed(NOW_TEXT))
                .update_personal_details(PERSONAL_ID, &json!(expected), &null_request())
                .await,
            "conflict",
        );
        assert_eq!(
            envelope(&error),
            json!({
                "code": "conflict",
                "message": "Profile revision conflict",
                "details": {"resource": "profile", "profileId": PERSONAL_ID, "expectedRevision": expected.to_string()}
            })
        );
        assert_eq!(repo.writes(), 0);
    }
}

#[tokio::test]
async fn compare_and_set_loss_after_read_is_a_profile_conflict() {
    let repo = fresh_personal();
    repo.lose_next_write();
    let error = expect_err(
        personal_service(&repo, fixed(NOW_TEXT))
            .update_personal_details(PERSONAL_ID, &json!(0), &null_request())
            .await,
        "lost race",
    );
    assert_eq!(envelope(&error)["code"], json!("conflict"));
    assert_eq!(
        repo.current().map(|envelope| envelope.profile_revision),
        Some(0)
    );
}

#[tokio::test]
async fn backward_clock_clamps_timestamps_and_keeps_the_photo() {
    let repo = fresh_personal();
    repo.set_envelope(PersonalDetailsEnvelope {
        profile_id: personal_id(),
        profile_revision: 0,
        profile_updated_at: at(LATER_TEXT),
        personal_details: Some(PersonalDetailsRecord {
            profile_id: personal_id(),
            date_of_birth: None,
            nationality: Some("Canada".to_owned()),
            photo_document_id: Some(Uuid::from_u128(2)),
            created_at: at(LATER_TEXT),
            updated_at: at(LATER_TEXT),
        }),
    });
    let result = expect_ok(
        personal_service(&repo, fixed(NOW_TEXT))
            .update_personal_details(PERSONAL_ID, &json!(0), &null_request())
            .await,
        "clamped",
    );
    let value = to_value(&result);
    assert_eq!(value["profileUpdatedAt"], json!(LATER_TEXT));
    assert_eq!(value["personalDetails"]["createdAt"], json!(LATER_TEXT));
    assert_eq!(
        value["personalDetails"]["photoDocumentId"],
        json!(Uuid::from_u128(2).to_string())
    );
}

#[tokio::test]
async fn raw_nationality_budget_boundary() {
    for nationality in ["x".repeat(1_000_000), "\u{1F600}".repeat(500_000)] {
        let repo = fresh_personal();
        let result = expect_ok(
            personal_service(&repo, fixed(NOW_TEXT))
                .update_personal_details(
                    PERSONAL_ID,
                    &json!(0),
                    &json!({"dateOfBirth": null, "nationality": nationality}),
                )
                .await,
            "budget boundary",
        );
        assert_eq!(
            result
                .personal_details
                .and_then(|details| details.nationality),
            Some(nationality)
        );
    }
    for nationality in [" ".repeat(1_000_001), "\u{1F600}".repeat(500_001)] {
        let repo = fresh_personal();
        let error = expect_err(
            personal_service(&repo, fixed(NOW_TEXT))
                .update_personal_details(
                    PERSONAL_ID,
                    &json!(0),
                    &json!({"dateOfBirth": null, "nationality": nationality}),
                )
                .await,
            "over budget",
        );
        assert_eq!(
            envelope(&error)["message"],
            json!("Resume preparation limit exceeded")
        );
        assert_eq!(repo.io_count(), 0);
    }
}

#[tokio::test]
async fn stale_revision_is_a_conflict_before_any_write() {
    let repo = MemoryPersonal::with_profile(personal_id(), 3, at(NOW_TEXT));
    let error = expect_err(
        personal_service(&repo, fixed(NOW_TEXT))
            .update_personal_details(PERSONAL_ID, &json!(2), &null_request())
            .await,
        "stale",
    );
    assert_eq!(envelope(&error)["code"], json!("conflict"));
    assert_eq!(repo.writes(), 0);
}

// ---- identifier admission ----

/// A valid identifier in lower case; [`refused_spellings`] wraps it in the refused forms.
const ADMISSION_ID: &str = "0f0e0d0c-0b0a-4908-8706-050403020100";
const UPPER_CONTACT_ID: &str = "ABCDEF01-2345-4678-9ABC-DEF012345678";
const LOWER_CONTACT_ID: &str = "abcdef01-2345-4678-9abc-def012345678";
const UPPER_LINK_ID: &str = "ABCDEF02-2345-4678-9ABC-DEF012345678";
const LOWER_LINK_ID: &str = "abcdef02-2345-4678-9abc-def012345678";
const UPPER_PROJECT_ID: &str = "ABCDEF03-2345-4678-9ABC-DEF012345678";
const LOWER_PROJECT_ID: &str = "abcdef03-2345-4678-9abc-def012345678";

/// Every identifier-bearing write-request path, with the field that its refusal reports.
const CHILD_ID_PATHS: [(&str, &str); 8] = [
    ("/contacts/0/id", "contacts[0].id"),
    ("/links/0/id", "links[0].id"),
    ("/skills/0/id", "skills[0].id"),
    ("/languages/0/id", "languages[0].id"),
    ("/education/0/id", "education[0].id"),
    ("/experiences/0/id", "experiences[0].id"),
    ("/projects/0/id", "projects[0].id"),
    (
        "/projects/0/technologies/0/id",
        "projects[0].technologies[0].id",
    ),
];

/// The spellings that are refused: braced (lower and upper case), URN, the 32-digit simple form, a
/// misplaced hyphen (36 bytes, hyphen at index 9), and the canonical text with a leading space, a
/// trailing space, a tab or a trailing newline.
fn refused_spellings() -> Vec<String> {
    vec![
        format!("{{{ADMISSION_ID}}}"),
        format!("{{{}}}", ADMISSION_ID.to_uppercase()),
        format!("urn:uuid:{ADMISSION_ID}"),
        ADMISSION_ID.replace('-', ""),
        String::from("0f0e0d0c0-b0a-4908-8706-050403020100"),
        format!(" {ADMISSION_ID}"),
        format!("{ADMISSION_ID} "),
        format!("\t{ADMISSION_ID}"),
        format!("{ADMISSION_ID}\n"),
    ]
}

/// The envelope of a malformed identifier in a profile request (`Invalid profile request`).
fn malformed_id(field: &str) -> Value {
    json!({
        "code": "validation_error",
        "message": "Invalid profile request",
        "details": {"field": field, "reason": "must be a valid UUID"}
    })
}

/// Sets the value at a JSON pointer of a request built by [`profile_request`].
fn set_at(request: &mut Value, pointer: &str, value: Value) {
    match request.pointer_mut(pointer) {
        Some(slot) => *slot = value,
        None => panic!("no value at {pointer}"),
    }
}

#[tokio::test]
async fn spelling_refuses_other_spellings_of_a_profile_id_on_get_update_and_patch() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = created.profile.id.to_string();
    for spelling in refused_spellings() {
        let get = expect_err(svc.get_profile(&spelling).await, "get");
        assert_eq!(
            envelope(&get),
            malformed_id("profileId"),
            "get of {spelling:?}"
        );
        let update = expect_err(
            svc.update_profile(&spelling, &json!(0), &profile_request())
                .await,
            "update",
        );
        assert_eq!(
            envelope(&update),
            malformed_id("profileId"),
            "update of {spelling:?}"
        );
        let patch = expect_err(
            svc.update_profile_project(&json!({
                "profileId": spelling.as_str(),
                "projectId": PROJECT_ID,
                "expectedRevision": 0,
                "name": "Renamed"
            }))
            .await,
            "project patch",
        );
        assert_eq!(
            envelope(&patch),
            malformed_id("profileId"),
            "patch of {spelling:?}"
        );
    }
    assert_eq!(repo.creates(), 1);
    assert_eq!(repo.replaces(), 0);
    assert_eq!(repo.project_updates(), 0);
    assert_eq!(
        expect_ok(svc.get_profile(&profile_id).await, "get"),
        created
    );
}

#[tokio::test]
async fn spelling_refuses_other_spellings_of_a_project_id_in_a_patch() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = created.profile.id.to_string();
    for spelling in refused_spellings() {
        let error = expect_err(
            svc.update_profile_project(&json!({
                "profileId": profile_id.as_str(),
                "projectId": spelling.as_str(),
                "expectedRevision": 0,
                "name": "Renamed"
            }))
            .await,
            "project id",
        );
        assert_eq!(
            envelope(&error),
            malformed_id("projectId"),
            "project id {spelling:?}"
        );
    }
    assert_eq!(repo.project_updates(), 0);
}

#[tokio::test]
async fn spelling_refuses_other_spellings_of_child_ids_in_write_requests() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = created.profile.id.to_string();
    for (pointer, field) in CHILD_ID_PATHS {
        for spelling in refused_spellings() {
            let mut request = profile_request();
            set_at(&mut request, pointer, json!(spelling.as_str()));
            let create = expect_err(svc.create_profile(&request).await, "create");
            assert_eq!(
                envelope(&create),
                malformed_id(field),
                "create {field} {spelling:?}"
            );
            let update = expect_err(
                svc.update_profile(&profile_id, &json!(0), &request).await,
                "update",
            );
            assert_eq!(
                envelope(&update),
                malformed_id(field),
                "update {field} {spelling:?}"
            );
        }
    }
    assert_eq!(repo.creates(), 1);
    assert_eq!(repo.replaces(), 0);
}

#[tokio::test]
async fn spelling_refuses_other_spellings_of_a_technology_id_in_a_patch() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = created.profile.id.to_string();
    for spelling in refused_spellings() {
        let error = expect_err(
            svc.update_profile_project(&json!({
                "profileId": profile_id.as_str(),
                "projectId": PROJECT_ID,
                "expectedRevision": 0,
                "technologies": [{
                    "id": spelling.as_str(),
                    "technology": "SQLite",
                    "displayOrder": 0
                }]
            }))
            .await,
            "technology id",
        );
        assert_eq!(
            envelope(&error),
            malformed_id("technologies[0].id"),
            "technology id {spelling:?}"
        );
    }
    assert_eq!(repo.project_updates(), 0);
}

#[tokio::test]
async fn spelling_refuses_other_spellings_of_the_profile_id_in_personal_details() {
    let repo = fresh_personal();
    let svc = personal_service(&repo, fixed(NOW_TEXT));
    for spelling in refused_spellings() {
        let get = expect_err(svc.get_personal_details(&spelling).await, "personal get");
        assert_eq!(
            envelope(&get),
            malformed_id("profileId"),
            "get of {spelling:?}"
        );
        let update = expect_err(
            svc.update_personal_details(&spelling, &json!(0), &null_request())
                .await,
            "personal update",
        );
        assert_eq!(
            envelope(&update),
            malformed_id("profileId"),
            "update of {spelling:?}"
        );
    }
    assert_eq!(repo.io_count(), 0);
}

#[tokio::test]
async fn spelling_accepts_uppercase_hyphenated_ids_and_normalizes_them_to_lowercase() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let mut request = profile_request();
    set_at(&mut request, "/contacts/0/id", json!(UPPER_CONTACT_ID));
    set_at(&mut request, "/links/0/id", json!(UPPER_LINK_ID));
    set_at(&mut request, "/projects/0/id", json!(UPPER_PROJECT_ID));
    let created = expect_ok(svc.create_profile(&request).await, "create");
    let value = to_value(&created);
    assert_eq!(value["contacts"][0]["id"], json!(LOWER_CONTACT_ID));
    assert_eq!(value["links"][0]["id"], json!(LOWER_LINK_ID));
    assert_eq!(value["projects"][0]["id"], json!(LOWER_PROJECT_ID));

    let profile_id = created.profile.id.to_string();
    let upper_profile = profile_id.to_uppercase();
    assert_eq!(
        expect_ok(svc.get_profile(&upper_profile).await, "get"),
        created
    );

    let patched = to_value(&expect_ok(
        svc.update_profile_project(&json!({
            "profileId": upper_profile.as_str(),
            "projectId": UPPER_PROJECT_ID,
            "expectedRevision": 0,
            "name": "Renamed"
        }))
        .await,
        "project patch",
    ));
    assert_eq!(patched["project"]["id"], json!(LOWER_PROJECT_ID));
    assert_eq!(patched["profileRevision"], json!(1));

    let updated = to_value(&expect_ok(
        svc.update_profile(&upper_profile, &json!(1), &profile_request())
            .await,
        "update",
    ));
    assert_eq!(updated["profile"]["id"], json!(profile_id));
    assert_eq!(repo.creates(), 1);
    assert_eq!(repo.project_updates(), 1);
    assert_eq!(repo.replaces(), 1);
}

#[tokio::test]
async fn spelling_accepts_uppercase_ids_in_every_write_request_path_and_normalizes_them() {
    let upper = ADMISSION_ID.to_uppercase();
    for (pointer, _) in CHILD_ID_PATHS {
        let mut request = profile_request();
        set_at(&mut request, pointer, json!(upper.as_str()));

        let create_repo = MemoryProfiles::new();
        let created = expect_ok(
            service(&create_repo, fixed(NOW_TEXT))
                .create_profile(&request)
                .await,
            "create",
        );
        assert_eq!(
            to_value(&created).pointer(pointer),
            Some(&json!(ADMISSION_ID)),
            "create {pointer}"
        );

        let update_repo = MemoryProfiles::new();
        let update_svc = service(&update_repo, fixed(NOW_TEXT));
        let existing = expect_ok(
            update_svc.create_profile(&profile_request()).await,
            "create existing",
        );
        let profile_id = existing.profile.id.to_string();
        let updated = expect_ok(
            update_svc
                .update_profile(&profile_id, &json!(0), &request)
                .await,
            "update",
        );
        assert_eq!(
            to_value(&updated).pointer(pointer),
            Some(&json!(ADMISSION_ID)),
            "update {pointer}"
        );
    }
}

#[tokio::test]
async fn spelling_accepts_uppercase_technology_id_in_a_patch() {
    let repo = MemoryProfiles::new();
    let svc = service(&repo, fixed(NOW_TEXT));
    let created = expect_ok(svc.create_profile(&profile_request()).await, "create");
    let profile_id = created.profile.id.to_string();
    let patched = to_value(&expect_ok(
        svc.update_profile_project(&json!({
            "profileId": profile_id.as_str(),
            "projectId": PROJECT_ID,
            "expectedRevision": 0,
            "technologies": [{
                "id": ADMISSION_ID.to_uppercase(),
                "technology": "SQLite",
                "displayOrder": 0
            }]
        }))
        .await,
        "technology patch",
    ));
    // The supplied technology identifier is admitted but never persisted (see
    // `project_patch_never_persists_a_supplied_technology_identifier`), so only the admission
    // and the write are pinned here, not the identifier value.
    assert_eq!(
        patched["project"]["technologies"][0]["technology"],
        json!("SQLite")
    );
    assert_ne!(
        patched["project"]["technologies"][0]["id"],
        json!(ADMISSION_ID.to_uppercase())
    );
    assert_eq!(repo.project_updates(), 1);
}

#[tokio::test]
async fn spelling_accepts_uppercase_profile_id_in_personal_details_and_normalizes_it() {
    let repo = fresh_personal();
    let svc = personal_service(&repo, fixed(NOW_TEXT));
    let upper = PERSONAL_ID.to_uppercase();
    let read = to_value(&expect_ok(
        svc.get_personal_details(&upper).await,
        "personal get",
    ));
    assert_eq!(read["profileId"], json!(PERSONAL_ID));
    let written = to_value(&expect_ok(
        svc.update_personal_details(&upper, &json!(0), &null_request())
            .await,
        "personal update",
    ));
    assert_eq!(written["profileId"], json!(PERSONAL_ID));
}
