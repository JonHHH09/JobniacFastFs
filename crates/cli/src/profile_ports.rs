//! The MCP profile ports over the PostgreSQL-backed profile use cases.
//!
//! The composition root is the only place that knows both the MCP ports (`jobniac-mcp`) and the
//! concrete use cases (`jobniac-core` over `jobniac-infra`). The use cases are generic over their
//! unit of work, and their transaction futures carry no `Send` bound in the core trait, so the MCP
//! crate cannot implement its `async` ports for them. Each adapter here wraps one concrete use case
//! and converts its typed result into the service dictionary the tools return.

use std::sync::Arc;

use async_trait::async_trait;
use jobniac_core::errors::AppError;
use jobniac_core::profiles::{ProfileListService, ProfileSearchService, ProfileService};
use jobniac_infra::postgres::profiles::PgProfileRepository;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_mcp::tools::profiles::{ProfileDetailsPort, ProfileListPort, ProfileSearchPort};
use jobniac_runtime::services::ProfileServices;
use serde::Serialize;
use serde_json::Value;

/// The profile aggregate use case over PostgreSQL.
pub type DetailsUseCase = ProfileService<PgUnitOfWork, PgProfileRepository>;
/// The keyset listing use case over PostgreSQL.
pub type ListUseCase = ProfileListService<PgUnitOfWork, PgProfileRepository>;
/// The bounded search use case over PostgreSQL.
pub type SearchUseCase = ProfileSearchService<PgUnitOfWork, PgProfileRepository>;

/// The MCP ports of the profile tools. An absent port leaves its tools unregistered.
#[derive(Debug, Clone, Default)]
pub struct ProfilePorts {
    /// The listing behind `list_profiles`.
    pub list: Option<Arc<dyn ProfileListPort>>,
    /// The aggregate use cases behind the four profile write and read tools.
    pub details: Option<Arc<dyn ProfileDetailsPort>>,
    /// The search behind `search_profiles`.
    pub search: Option<Arc<dyn ProfileSearchPort>>,
}

/// The ports of the composed profile services. A listing whose cursor key is invalid has no port,
/// so `list_profiles` is absent and the other tools still work.
pub fn from_services(profiles: &ProfileServices) -> ProfilePorts {
    build(
        Arc::clone(&profiles.details),
        profiles.list.as_ref().ok().cloned(),
        Arc::clone(&profiles.search),
    )
}

/// The ports over the given use cases.
fn build(
    details: Arc<DetailsUseCase>,
    list: Option<Arc<ListUseCase>>,
    search: Arc<SearchUseCase>,
) -> ProfilePorts {
    ProfilePorts {
        list: list.map(|use_case| Arc::new(ListAdapter(use_case)) as Arc<dyn ProfileListPort>),
        details: Some(Arc::new(DetailsAdapter(details)) as Arc<dyn ProfileDetailsPort>),
        search: Some(Arc::new(SearchAdapter(search)) as Arc<dyn ProfileSearchPort>),
    }
}

/// Converts a use-case result into its service dictionary. A failure to serialize is internal.
pub(crate) fn to_dictionary<T: Serialize>(value: &T) -> Result<Value, AppError> {
    serde_json::to_value(value).map_err(|_| AppError::Internal)
}

/// `list_profiles` over the keyset listing.
#[derive(Debug)]
struct ListAdapter(Arc<ListUseCase>);

#[async_trait]
impl ProfileListPort for ListAdapter {
    async fn list_profiles(
        &self,
        limit: Option<i64>,
        cursor: Option<&str>,
    ) -> Result<Value, AppError> {
        to_dictionary(&self.0.list_profiles(limit, cursor).await?)
    }
}

/// The four profile tools over the aggregate use case.
#[derive(Debug)]
struct DetailsAdapter(Arc<DetailsUseCase>);

#[async_trait]
impl ProfileDetailsPort for DetailsAdapter {
    async fn get_profile(&self, profile_id: &str) -> Result<Value, AppError> {
        to_dictionary(&self.0.get_profile(profile_id).await?)
    }

    async fn create_profile(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.create_profile(request).await?)
    }

    async fn update_profile(
        &self,
        profile_id: &str,
        expected_revision: i64,
        request: &Value,
    ) -> Result<Value, AppError> {
        let expected = Value::from(expected_revision);
        to_dictionary(
            &self
                .0
                .update_profile(profile_id, &expected, request)
                .await?,
        )
    }

    async fn update_profile_project(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.update_profile_project(request).await?)
    }
}

/// `search_profiles` over the bounded search.
#[derive(Debug)]
struct SearchAdapter(Arc<SearchUseCase>);

#[async_trait]
impl ProfileSearchPort for SearchAdapter {
    async fn search_profiles(&self, request: &Value) -> Result<Value, AppError> {
        to_dictionary(&self.0.search_profiles(request).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;

    use jobniac_core::clock::SystemClock;
    use jobniac_core::pagination::{CursorCodec, CursorKey};
    use jobniac_core::profiles::{ProfileListService, ProfileSearchService, ProfileService};
    use jobniac_infra::postgres::config::load_database_config;
    use jobniac_infra::postgres::migrations::migrate;
    use jobniac_infra::postgres::pool::PgPool;
    use jobniac_infra::postgres::profiles::PgProfileRepository;
    use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
    use jobniac_mcp::catalog::Catalog;
    use jobniac_mcp::registry::{Arguments, ToolRegistry};
    use jobniac_mcp::result::ToolResult;
    use jobniac_mcp::tools::profiles;
    use jobniac_testkit::pg::TestServer;
    use serde_json::{Value, json};

    use super::build;

    type TestResult = Result<(), Box<dyn Error>>;

    const CURSOR_SECRET: &str = "profile-ports-cursor-secret-0123456789";

    /// Calls one tool through the registry and returns its outcome.
    async fn call(
        registry: &ToolRegistry,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, String> {
        let Value::Object(map) = arguments else {
            return Err("arguments must be an object".to_owned());
        };
        let map: Arguments = map;
        registry
            .call(name, &map)
            .await
            .ok_or_else(|| format!("{name} is not registered"))
    }

    /// The structured success of a call, or the failure as a message.
    fn success(outcome: ToolResult) -> Result<Value, String> {
        match outcome {
            ToolResult::Success(value) => Ok(value),
            other => Err(format!("expected a success, got {other:?}")),
        }
    }

    /// The `code` of a failure envelope, or the outcome as a message.
    fn failure_code(outcome: ToolResult) -> Result<String, String> {
        match outcome {
            ToolResult::Failure { envelope, .. } => Ok(envelope.code.code().to_owned()),
            other => Err(format!("expected a failure, got {other:?}")),
        }
    }

    fn write_request(full_name: &str, email: &str) -> Value {
        json!({
            "fullName": full_name,
            "email": email,
            "summary": "Synthetic summary",
            "contacts": [{
                "id": null, "contactType": "email", "contactValue": email, "label": null
            }],
            "links": null,
            "skills": [{
                "id": null, "skill": "Rust", "normalizedSkill": "rust",
                "category": "Languages", "displayOrder": 0
            }],
            "languages": null,
            "education": null,
            "experiences": null,
            "projects": [{
                "id": null, "name": "Demo project", "url": null,
                "description": null, "displayOrder": 0,
                "technologies": [{
                    "id": null, "technology": "Rust",
                    "normalizedTechnology": "rust", "displayOrder": 0
                }]
            }],
        })
    }

    /// The profile tools through the MCP handler layer, over a disposable migrated database.
    /// Skips with a notice when no PostgreSQL server is configured for the tests.
    #[tokio::test]
    async fn profile_tools_create_read_update_search_and_list_through_the_mcp_handlers()
    -> TestResult {
        let Some(server) = TestServer::from_env() else {
            eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
            return Ok(());
        };
        let database = server.fresh_database().await;
        let config = load_database_config(&database.environment())
            .ok_or("the disposable database configuration is invalid")?;
        migrate(&config).await.map_err(|error| error.0)?;

        let uow = PgUnitOfWork::new(PgPool::new(Some(config)));
        let details = Arc::new(ProfileService::new(
            uow.clone(),
            PgProfileRepository,
            Arc::new(SystemClock),
        ));
        let search = Arc::new(ProfileSearchService::new(uow.clone(), PgProfileRepository));
        let codec = CursorCodec::new(CursorKey::from_secret(CURSOR_SECRET)?);
        let list = Arc::new(ProfileListService::new(
            uow.clone(),
            PgProfileRepository,
            codec,
        ));
        let ports = build(details, Some(list), search);
        let mut registry = ToolRegistry::new(Catalog::load()?);
        profiles::register(&mut registry, ports.list, ports.details, ports.search)?;

        // Create: the persisted graph comes back with its identifier and revision.
        let created = success(
            call(
                &registry,
                "create_profile",
                json!({ "request": write_request("Example Candidate", "candidate@example.test") }),
            )
            .await?,
        )?;
        let profile_id = created["profile"]["id"]
            .as_str()
            .ok_or("no profile id")?
            .to_owned();
        let revision = created["profile"]["revision"]
            .as_i64()
            .ok_or("no revision")?;

        // Get: the identifier is accepted in uppercase and the graph matches the write.
        let fetched = success(
            call(
                &registry,
                "get_profile",
                json!({ "profileId": profile_id.to_ascii_uppercase() }),
            )
            .await?,
        )?;
        assert_eq!(fetched["profile"]["fullName"], "Example Candidate");
        assert_eq!(
            fetched["projects"][0]["technologies"][0]["technology"],
            "Rust"
        );

        // Update: the revision check accepts the current revision and advances it by one.
        let updated = success(
            call(
                &registry,
                "update_profile",
                json!({
                    "profileId": profile_id,
                    "expectedRevision": revision,
                    "request": write_request("Renamed Candidate", "renamed@example.test"),
                }),
            )
            .await?,
        )?;
        assert_eq!(updated["profile"]["fullName"], "Renamed Candidate");
        let new_revision = updated["profile"]["revision"]
            .as_i64()
            .ok_or("no revision")?;
        assert_eq!(new_revision, revision + 1);

        // A stale revision is a conflict, and nothing is written.
        let stale = call(
            &registry,
            "update_profile",
            json!({
                "profileId": profile_id,
                "expectedRevision": revision,
                "request": write_request("Stale Candidate", "stale@example.test"),
            }),
        )
        .await?;
        assert_eq!(failure_code(stale)?, "conflict");

        // Project patch: omitted fields stay, and the profile revision advances. The replace above
        // gave the project a new identifier, so the patch uses that one.
        let project_id = updated["projects"][0]["id"]
            .as_str()
            .ok_or("no project id after the update")?
            .to_owned();
        assert_ne!(
            Some(project_id.as_str()),
            created["projects"][0]["id"].as_str()
        );
        let patched = success(
            call(
                &registry,
                "update_profile_project",
                json!({ "request": {
                    "profileId": profile_id,
                    "projectId": project_id,
                    "expectedRevision": new_revision,
                    "name": "Renamed project",
                    "technologies": null,
                } }),
            )
            .await?,
        )?;
        assert_eq!(patched["profileRevision"], new_revision + 1);
        assert_eq!(patched["project"]["name"], "Renamed project");

        // Search finds the profile by a token of its name.
        let found = success(
            call(
                &registry,
                "search_profiles",
                json!({ "request": { "query": "renamed", "limit": 5 } }),
            )
            .await?,
        )?;
        let matches = found["profiles"].as_array().ok_or("no profiles array")?;
        assert!(
            matches
                .iter()
                .any(|hit| hit["profile"]["id"] == profile_id.as_str()),
            "the search must return the renamed profile: {found}"
        );

        // List: the first page holds the profile and no cursor follows it.
        let listed = success(
            call(
                &registry,
                "list_profiles",
                json!({ "request": { "limit": 10 } }),
            )
            .await?,
        )?;
        let profiles = listed["profiles"].as_array().ok_or("no profiles array")?;
        assert!(
            profiles
                .iter()
                .any(|profile| profile["id"] == profile_id.as_str())
        );
        assert!(listed["nextCursor"].is_null());

        // A rejected write never reaches the database: the email is refused by the use case.
        let invalid = call(
            &registry,
            "create_profile",
            json!({ "request": write_request("Invalid Candidate", "not-an-email") }),
        )
        .await?;
        assert_eq!(failure_code(invalid)?, "validation_error");
        Ok(())
    }
}
