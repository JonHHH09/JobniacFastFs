//! Classified generated PDFs against a disposable PostgreSQL 18 server: reservation, publication,
//! finalization, abandonment, bounded recovery and the reconciliation of lost acknowledgments
//! The file port is a recording fake
//! with fault hooks, and a test-only repository wrapper loses an acknowledgment after the commit.
//! Database faults are triggers installed by the shared `german_support` helpers. The server-backed
//! tests skip with a notice when `JOBNIAC_TEST_PG_PORT` is unset and fail on CI. All data is synthetic.

mod german_support;

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use jobniac_core::cleanup::CleanupRepository;
use jobniac_core::clock::{Clock, truncate_to_micros};
use jobniac_core::errors::{AppError, ApplicationError};
use jobniac_core::generated_assets::service::{
    GeneratedAssetRepository, GeneratedAssetService, ReserveError,
};
use jobniac_core::generated_assets::standalone::{
    StandaloneAssetRepository, StandaloneAssetService,
};
use jobniac_core::generated_assets::{
    AssetIdentity, GENERATION_LEASE, GeneratedAssetWriter, GenerationDescriptor, GenerationPurpose,
    IntentReservation, MaterializationOutcome, PreparationOutcome, PublicationFailure,
    PublicationFailureCategory, PublicationOutcome, RETRY_BATCH_SIZE,
};
use jobniac_core::transactions::TransactionOptions;
use jobniac_infra::postgres::cleanup::PgCleanupRepository;
use jobniac_infra::postgres::config::{DatabaseConfig, load_database_config};
use jobniac_infra::postgres::generated::repository::PgGeneratedAssetRepository;
use jobniac_infra::postgres::migrations::migrate;
use jobniac_infra::postgres::pool::PgPool;
use jobniac_infra::postgres::unit_of_work::PgUnitOfWork;
use jobniac_testkit::pg::{TestDatabase, TestServer};
use time::{Duration, OffsetDateTime};
use tokio_postgres::Client;
use uuid::Uuid;

use german_support::fail_at;

type TestResult = Result<(), Box<dyn Error>>;

/// A synthetic PDF with a valid header; the same bytes are published in every test.
const PDF: &[u8] = b"%PDF-1.7\n% synthetic classified resume\n";

/// The shared instant of every test; the injected clock moves only when a test advances it.
fn start() -> OffsetDateTime {
    truncate_to_micros(
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap_or(OffsetDateTime::UNIX_EPOCH),
    )
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A clock that moves only when a test advances it.
#[derive(Debug)]
struct TestClock(Mutex<OffsetDateTime>);

impl TestClock {
    fn new(at: OffsetDateTime) -> Arc<Self> {
        Arc::new(Self(Mutex::new(at)))
    }

    fn advance(&self, by: Duration) {
        *lock(&self.0) += by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> OffsetDateTime {
        *lock(&self.0)
    }
}

/// A publication hook that runs inside the file port, before the failure (if any) is returned.
type Hook = Box<dyn Fn() + Send + Sync>;

/// File fake that records every publication, runs an optional hook and fails with an optional
/// category (the failure a hook raises is a `PublicationFailure`).
struct Writer {
    failure: Option<PublicationFailureCategory>,
    hook: Option<Hook>,
    calls: Mutex<Vec<Uuid>>,
}

impl Writer {
    fn new(failure: Option<PublicationFailureCategory>, hook: Option<Hook>) -> Arc<Self> {
        Arc::new(Self {
            failure,
            hook,
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<Uuid> {
        lock(&self.calls).clone()
    }
}

impl GeneratedAssetWriter for Writer {
    fn materialize_owned(
        &self,
        identity: &AssetIdentity,
        _content: &[u8],
    ) -> Result<PublicationOutcome, PublicationFailure> {
        lock(&self.calls).push(identity.asset_id());
        if let Some(hook) = &self.hook {
            hook();
        }
        match self.failure {
            Some(category) => Err(PublicationFailure { category }),
            None => Ok(PublicationOutcome::Created),
        }
    }
}

/// A disposable migrated database, its configuration and an admin session for raw checks.
struct Fixture {
    database: TestDatabase,
    config: DatabaseConfig,
    uow: PgUnitOfWork,
}

/// A fresh migrated database, or `None` without a configured server (skip locally, fail on CI).
async fn fixture() -> Result<Option<Fixture>, Box<dyn Error>> {
    let Some(server) = TestServer::from_env() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "JOBNIAC_TEST_PG_PORT must be set in CI so that the classified PostgreSQL tests run"
        );
        eprintln!("skipped: JOBNIAC_TEST_PG_PORT is not set");
        return Ok(None);
    };
    let database = server.fresh_database().await;
    let config = load_database_config(&database.environment())
        .ok_or("the disposable database configuration is invalid")?;
    migrate(&config).await.map_err(|error| error.0)?;
    let uow = PgUnitOfWork::new(PgPool::new(Some(config.clone())));
    Ok(Some(Fixture {
        database,
        config,
        uow,
    }))
}

/// The service and the repository over one fixture, with the injected clock and file fake.
struct Env {
    fixture: Fixture,
    root: Uuid,
    repository: Arc<PgGeneratedAssetRepository>,
    service: GeneratedAssetService,
}

/// Registers a generated root and wires the service over the PostgreSQL repository.
async fn environment(
    fixture: Fixture,
    clock: Arc<TestClock>,
    writer: Arc<Writer>,
) -> Result<Env, Box<dyn Error>> {
    let root = Uuid::new_v4();
    let now = clock.now();
    fixture
        .uow
        .transaction::<(), AppError>(TransactionOptions::default(), async |tx| {
            tx.client()
                .execute(
                    "INSERT INTO jobniac.generated_asset_roots (id, created_at) VALUES ($1, $2)",
                    &[&root, &now],
                )
                .await
                .map_err(|_| AppError::Internal)?;
            Ok(())
        })
        .await?;
    let repository = Arc::new(PgGeneratedAssetRepository::new(
        fixture.uow.clone(),
        clock.clone(),
    ));
    let service = GeneratedAssetService::new(
        repository.clone() as Arc<dyn GeneratedAssetRepository>,
        writer.clone() as Arc<dyn GeneratedAssetWriter>,
        root,
        clock.clone(),
        true,
    );
    Ok(Env {
        fixture,
        root,
        repository,
        service,
    })
}

/// A descriptor of one purpose, with the fields that purpose requires.
fn descriptor(
    purpose: GenerationPurpose,
    scope: Option<&str>,
) -> Result<GenerationDescriptor, Box<dyn Error>> {
    let operation = Uuid::new_v4();
    let scope = scope.map(str::to_owned);
    let descriptor = match purpose {
        GenerationPurpose::StandalonePdf => {
            GenerationDescriptor::new(operation, purpose, None, None, None, None)
        }
        GenerationPurpose::ProfileResume => GenerationDescriptor::new(
            operation,
            purpose,
            Some(Uuid::new_v4()),
            None,
            None,
            scope.or(Some("master_resume".to_owned())),
        ),
        GenerationPurpose::GermanResume | GenerationPurpose::GermanCoverLetter => {
            GenerationDescriptor::new(
                operation,
                purpose,
                Some(Uuid::new_v4()),
                Some(Uuid::new_v4()),
                Some(Uuid::new_v4()),
                scope,
            )
        }
    };
    Ok(descriptor?)
}

/// The stored state of one asset and its intent and cleanup rows.
#[derive(Debug, PartialEq, Eq)]
struct State {
    lifecycle: String,
    intent_state: Option<String>,
    lease_token: Option<Uuid>,
    lease_expires_at: Option<OffsetDateTime>,
    finished_at: Option<OffsetDateTime>,
    status: Option<String>,
}

async fn state(admin: &Client, asset: Uuid) -> Result<State, Box<dyn Error>> {
    let row = admin
        .query_one(
            "SELECT asset.lifecycle::text AS lifecycle, intent.state::text AS intent_state, \
             intent.lease_token, intent.lease_expires_at, intent.finished_at, \
             task.status::text AS status \
             FROM jobniac.generated_assets asset \
             LEFT JOIN jobniac.generation_intents intent ON intent.asset_id = asset.asset_id \
             LEFT JOIN jobniac.generated_asset_cleanups task ON task.asset_id = asset.asset_id \
             WHERE asset.asset_id = $1",
            &[&asset],
        )
        .await?;
    Ok(State {
        lifecycle: row.try_get("lifecycle")?,
        intent_state: row.try_get("intent_state")?,
        lease_token: row.try_get("lease_token")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        finished_at: row.try_get("finished_at")?,
        status: row.try_get("status")?,
    })
}

async fn count(admin: &Client, sql: &str) -> Result<i64, Box<dyn Error>> {
    Ok(admin.query_one(sql, &[]).await?.try_get(0)?)
}

/// The test-only repository that loses one acknowledgment: the operation commits, then its caller
/// receives a database refusal (the refusal is raised after the commit).
struct LostAcknowledgment {
    inner: Arc<PgGeneratedAssetRepository>,
    operation: &'static str,
    armed: AtomicBool,
}

impl LostAcknowledgment {
    fn lose(&self, operation: &str) -> Result<(), AppError> {
        if operation == self.operation && self.armed.swap(false, Ordering::SeqCst) {
            return Err(ApplicationError::database_unavailable().into());
        }
        Ok(())
    }
}

#[async_trait]
impl GeneratedAssetRepository for LostAcknowledgment {
    async fn reserve_classified_pdf(
        &self,
        identity: &AssetIdentity,
        document_id: Uuid,
        original_file_name: &str,
        content: &[u8],
        descriptor: &GenerationDescriptor,
        lease_token: Uuid,
        now: OffsetDateTime,
    ) -> Result<IntentReservation, AppError> {
        let reserved = GeneratedAssetRepository::reserve_classified_pdf(
            self.inner.as_ref(),
            identity,
            document_id,
            original_file_name,
            content,
            descriptor,
            lease_token,
            now,
        )
        .await?;
        self.lose("reserve")?;
        Ok(reserved)
    }

    async fn find_classified_reservation(
        &self,
        asset_id: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        GeneratedAssetRepository::find_classified_reservation(self.inner.as_ref(), asset_id).await
    }

    async fn materialize_classified(
        &self,
        claim: &IntentReservation,
        writer: &dyn GeneratedAssetWriter,
    ) -> Result<MaterializationOutcome, AppError> {
        let outcome =
            GeneratedAssetRepository::materialize_classified(self.inner.as_ref(), claim, writer)
                .await?;
        self.lose("materialize")?;
        Ok(outcome)
    }

    async fn renew_generation(
        &self,
        claim: &IntentReservation,
        new_token: Uuid,
    ) -> Result<Option<IntentReservation>, AppError> {
        GeneratedAssetRepository::renew_generation(self.inner.as_ref(), claim, new_token).await
    }

    async fn abandon_generation(
        &self,
        claim: &IntentReservation,
    ) -> Result<PreparationOutcome, AppError> {
        GeneratedAssetRepository::abandon_generation(self.inner.as_ref(), claim).await
    }

    async fn recover_expired_generations(
        &self,
        root_id: Uuid,
        limit: usize,
    ) -> Result<Vec<Uuid>, AppError> {
        GeneratedAssetRepository::recover_expired_generations(self.inner.as_ref(), root_id, limit)
            .await
    }
}

/// A second repository over the same database with a fresh pool, as after a process restart.
fn restarted(fixture: &Fixture, clock: &Arc<TestClock>) -> PgGeneratedAssetRepository {
    let pool = PgPool::new(Some(fixture.config.clone()));
    PgGeneratedAssetRepository::new(PgUnitOfWork::new(pool), clock.clone())
}

#[tokio::test]
async fn classified_reservation_has_exact_purpose_and_terminal_semantics() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let admin = env.fixture.database.admin_client().await;
    let cases = [
        (GenerationPurpose::StandalonePdf, None),
        (GenerationPurpose::ProfileResume, Some("master_resume")),
        (GenerationPurpose::ProfileResume, Some("canadian_resume")),
        (GenerationPurpose::ProfileResume, Some("canadian_resume_fr")),
        (GenerationPurpose::GermanResume, Some("en")),
        (GenerationPurpose::GermanResume, Some("de")),
        (GenerationPurpose::GermanCoverLetter, Some("de")),
    ];
    for (purpose, scope) in cases {
        let expected = descriptor(purpose, scope)?;
        let claim = env
            .service
            .reserve_classified_pdf(PDF, "prepared.pdf", &expected)
            .await?;
        let asset = claim.asset_id();
        let before = state(&admin, asset).await?;
        assert_eq!(before.lifecycle, "RESERVED");
        assert_eq!(before.intent_state.as_deref(), Some("PENDING"));
        assert_eq!(before.lease_expires_at, Some(start() + GENERATION_LEASE));

        let result = env.service.materialize_classified(&claim).await?;
        let after = state(&admin, asset).await?;
        assert_eq!(result.outcome, MaterializationOutcome::Active);
        assert_eq!(after.lifecycle, "ACTIVE");
        assert_eq!(result.claim.reservation, claim.reservation);
        assert_eq!(result.claim.intent.descriptor, expected);
        if purpose == GenerationPurpose::StandalonePdf {
            assert_eq!(after.intent_state.as_deref(), Some("COMPLETED"));
            assert!(after.lease_token.is_none() && after.lease_expires_at.is_none());
            assert_eq!(after.finished_at, Some(start()));
        } else {
            assert_eq!(after.intent_state.as_deref(), Some("PENDING"));
            assert_eq!(after.lease_token, claim.intent.lease_token);
            assert_eq!(result.claim, claim);
        }
    }
    assert_eq!(writer.calls().len(), cases.len());
    Ok(())
}

#[tokio::test]
async fn completed_standalone_is_durable_across_restart_and_arbitrary_age() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let admin = env.fixture.database.admin_client().await;
    let claim = env
        .service
        .reserve_classified_pdf(
            PDF,
            "retained.pdf",
            &descriptor(GenerationPurpose::StandalonePdf, None)?,
        )
        .await?;
    env.service.materialize_classified(&claim).await?;
    clock.advance(Duration::days(40));

    let restart = restarted(&env.fixture, &clock);
    assert!(
        restart
            .recover_expired_generations(env.root, RETRY_BATCH_SIZE)
            .await?
            .is_empty()
    );
    let cleanup = PgCleanupRepository::new(env.fixture.uow.clone(), clock.clone());
    assert_eq!(
        cleanup.prepare_cleanup(claim.asset_id()).await?,
        PreparationOutcome::Referenced
    );
    assert_eq!(
        restart.abandon_generation(&claim).await?,
        PreparationOutcome::Referenced
    );
    let calls_before = writer.calls().len();
    assert_eq!(
        restart
            .materialize_classified(&claim, writer.as_ref())
            .await?,
        MaterializationOutcome::Active
    );
    assert_eq!(
        writer.calls().len(),
        calls_before,
        "a replay never publishes again"
    );
    let after = state(&admin, claim.asset_id()).await?;
    assert_eq!(after.intent_state.as_deref(), Some("COMPLETED"));
    assert_eq!(after.lifecycle, "ACTIVE");
    assert_eq!(after.status, None);
    Ok(())
}

#[tokio::test]
async fn expired_incomplete_recovery_is_atomic_bounded_and_repeatable() -> TestResult {
    for publish in [false, true] {
        let Some(fixture) = fixture().await? else {
            return Ok(());
        };
        let clock = TestClock::new(start());
        let writer = Writer::new(None, None);
        let env = environment(fixture, clock.clone(), writer.clone()).await?;
        let admin = env.fixture.database.admin_client().await;
        let mut claims = Vec::new();
        for index in 0..3 {
            let name = format!("pending-{index}.pdf");
            let expected = descriptor(GenerationPurpose::ProfileResume, None)?;
            claims.push(
                env.service
                    .reserve_classified_pdf(PDF, &name, &expected)
                    .await?,
            );
        }
        if publish {
            for claim in &claims {
                env.service.materialize_classified(claim).await?;
            }
        }
        let cleanup = PgCleanupRepository::new(env.fixture.uow.clone(), clock.clone());
        for claim in &claims {
            assert_eq!(
                cleanup.prepare_cleanup(claim.asset_id()).await?,
                PreparationOutcome::Ineligible
            );
        }
        clock.advance(GENERATION_LEASE);
        let repository = env.repository.clone();
        let first = repository.recover_expired_generations(env.root, 2).await?;
        assert_eq!(first.len(), 2);
        let last = repository.recover_expired_generations(env.root, 2).await?;
        assert_eq!(last.len(), 1);
        assert!(first.iter().all(|asset| !last.contains(asset)));
        assert!(
            repository
                .recover_expired_generations(env.root, 2)
                .await?
                .is_empty()
        );
        for claim in &claims {
            let after = state(&admin, claim.asset_id()).await?;
            assert_eq!(after.intent_state.as_deref(), Some("ABANDONED"));
            assert_eq!(after.lifecycle, "RETIRING");
            assert_eq!(after.status.as_deref(), Some("PENDING"));
            assert!(after.lease_token.is_none() && after.lease_expires_at.is_none());
            assert_eq!(
                repository.abandon_generation(claim).await?,
                PreparationOutcome::AlreadyRetired
            );
        }
        assert_eq!(
            count(
                &admin,
                "SELECT count(*) FROM jobniac.generated_asset_cleanups"
            )
            .await?,
            3
        );
        assert_eq!(writer.calls().len(), if publish { 3 } else { 0 });
    }
    Ok(())
}

#[tokio::test]
async fn classified_recovery_does_not_adopt_unclassified_reserved_or_active_assets() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let admin = env.fixture.database.admin_client().await;
    let foundation = StandaloneAssetService::new(
        env.repository.clone() as Arc<dyn StandaloneAssetRepository>,
        writer.clone() as Arc<dyn GeneratedAssetWriter>,
        env.root,
        clock.clone(),
        true,
    );
    let reserved = foundation
        .reserve_pdf(PDF, "unclassified-reserved.pdf")
        .await?;
    let active = foundation
        .reserve_pdf(PDF, "unclassified-active.pdf")
        .await?;
    let reservation = active.clone();
    env.repository
        .materialize_reserved(reservation.asset.asset_id(), writer.as_ref())
        .await?;
    clock.advance(Duration::hours(1));
    assert!(
        env.repository
            .recover_expired_generations(env.root, RETRY_BATCH_SIZE)
            .await?
            .is_empty()
    );
    let reserved_id = reserved.asset.asset_id();
    let active_id = reservation.asset.asset_id();
    assert_eq!(state(&admin, reserved_id).await?.lifecycle, "RESERVED");
    assert_eq!(state(&admin, active_id).await?.lifecycle, "ACTIVE");
    assert_eq!(state(&admin, reserved_id).await?.intent_state, None);
    assert_eq!(state(&admin, active_id).await?.intent_state, None);
    Ok(())
}

#[tokio::test]
async fn renewed_reservation_excludes_foundation_age_recovery_and_old_fences() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let claim = env
        .service
        .reserve_classified_pdf(
            PDF,
            "renewed.pdf",
            &descriptor(GenerationPurpose::ProfileResume, None)?,
        )
        .await?;
    clock.advance(Duration::minutes(4));
    let renewed = env
        .service
        .renew_generation(&claim)
        .await?
        .ok_or("the live claim renews")?;
    assert_ne!(renewed.intent.lease_token, claim.intent.lease_token);
    clock.advance(Duration::minutes(2));

    let repository = env.repository.clone();
    let stale_before = clock.now() - jobniac_core::generated_assets::CLAIM_LEASE;
    assert!(
        repository
            .recover_stale_reservations(env.root, stale_before, RETRY_BATCH_SIZE)
            .await?
            .is_empty()
    );
    assert!(
        repository
            .recover_expired_generations(env.root, RETRY_BATCH_SIZE)
            .await?
            .is_empty()
    );
    assert_eq!(
        repository
            .materialize_classified(&claim, writer.as_ref())
            .await?,
        MaterializationOutcome::Ineligible
    );
    assert!(
        repository
            .renew_generation(&claim, Uuid::new_v4())
            .await?
            .is_none()
    );
    assert_eq!(
        repository.abandon_generation(&claim).await?,
        PreparationOutcome::Ineligible
    );
    assert_eq!(
        repository
            .materialize_reserved(claim.asset_id(), writer.as_ref())
            .await?,
        MaterializationOutcome::Ineligible
    );
    assert_eq!(
        env.service.materialize_classified(&renewed).await?.outcome,
        MaterializationOutcome::Active
    );
    assert_eq!(writer.calls(), vec![claim.asset_id()]);
    Ok(())
}

#[tokio::test]
async fn expired_fence_never_authorizes_publication_or_resurrection() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let admin = env.fixture.database.admin_client().await;
    let claim = env
        .service
        .reserve_classified_pdf(
            PDF,
            "expired.pdf",
            &descriptor(GenerationPurpose::StandalonePdf, None)?,
        )
        .await?;
    clock.advance(GENERATION_LEASE);
    assert_eq!(
        env.service.materialize_classified(&claim).await?.outcome,
        MaterializationOutcome::Ineligible
    );
    assert!(writer.calls().is_empty());
    assert!(env.service.renew_generation(&claim).await?.is_none());
    assert_eq!(
        env.service.abandon_generation(&claim).await?,
        PreparationOutcome::Prepared
    );
    assert_eq!(
        env.repository
            .materialize_classified(&claim, writer.as_ref())
            .await?,
        MaterializationOutcome::Ineligible
    );
    assert!(writer.calls().is_empty());
    assert_eq!(state(&admin, claim.asset_id()).await?.lifecycle, "RETIRING");
    Ok(())
}

#[tokio::test]
async fn publication_crossing_lease_expiry_cannot_commit_success() -> TestResult {
    for purpose in [
        GenerationPurpose::StandalonePdf,
        GenerationPurpose::ProfileResume,
    ] {
        let Some(fixture) = fixture().await? else {
            return Ok(());
        };
        let clock = TestClock::new(start());
        let late = clock.clone();
        let writer = Writer::new(None, Some(Box::new(move || late.advance(GENERATION_LEASE))));
        let env = environment(fixture, clock.clone(), writer.clone()).await?;
        let admin = env.fixture.database.admin_client().await;
        let claim = env
            .service
            .reserve_classified_pdf(PDF, "slow.pdf", &descriptor(purpose, None)?)
            .await?;
        let result = env.service.materialize_classified(&claim).await?;
        assert_eq!(result.outcome, MaterializationOutcome::CleanupPending);
        let after = state(&admin, claim.asset_id()).await?;
        assert_eq!(after.intent_state.as_deref(), Some("ABANDONED"));
        assert_eq!(after.lifecycle, "RETIRING");
        assert_eq!(after.status.as_deref(), Some("PENDING"));
    }
    Ok(())
}

#[tokio::test]
async fn failed_publication_commits_abandonment_and_queue_together() -> TestResult {
    for category in [
        PublicationFailureCategory::FileWriteFailed,
        PublicationFailureCategory::IdentityMismatch,
        PublicationFailureCategory::OwnershipUnproven,
        PublicationFailureCategory::RetiredAsset,
        PublicationFailureCategory::UnsupportedPlatform,
    ] {
        let Some(fixture) = fixture().await? else {
            return Ok(());
        };
        let clock = TestClock::new(start());
        let writer = Writer::new(Some(category), None);
        let env = environment(fixture, clock.clone(), writer.clone()).await?;
        let admin = env.fixture.database.admin_client().await;
        let claim = env
            .service
            .reserve_classified_pdf(
                PDF,
                "failed.pdf",
                &descriptor(GenerationPurpose::StandalonePdf, None)?,
            )
            .await?;
        let result = env.service.materialize_classified(&claim).await?;
        let retryable = category == PublicationFailureCategory::FileWriteFailed;
        let expected = if retryable {
            MaterializationOutcome::CleanupPending
        } else {
            MaterializationOutcome::Blocked
        };
        assert_eq!(result.outcome, expected, "{category:?}");
        let after = state(&admin, claim.asset_id()).await?;
        assert_eq!(after.intent_state.as_deref(), Some("ABANDONED"));
        assert_eq!(after.lifecycle, "RETIRING");
        assert_eq!(
            after.status.as_deref(),
            Some(if retryable { "PENDING" } else { "BLOCKED" })
        );
    }
    Ok(())
}

#[tokio::test]
async fn reservation_failure_rolls_back_document_blob_asset_and_intent() -> TestResult {
    for table in [
        "document.documents",
        "jobniac.generated_assets",
        "jobniac.generation_intents",
    ] {
        let Some(fixture) = fixture().await? else {
            return Ok(());
        };
        let clock = TestClock::new(start());
        let writer = Writer::new(None, None);
        let env = environment(fixture, clock.clone(), writer.clone()).await?;
        let admin = env.fixture.database.admin_client().await;
        fail_at(&admin, table, "INSERT", "TRUE").await?;
        let refused = env
            .service
            .reserve_classified_pdf(
                PDF,
                "rollback.pdf",
                &descriptor(GenerationPurpose::StandalonePdf, None)?,
            )
            .await;
        assert!(
            matches!(refused, Err(ReserveError::Unavailable(_))),
            "{table}"
        );
        for target in [
            "document.documents",
            "document.blobs",
            "jobniac.generated_assets",
            "jobniac.generation_intents",
        ] {
            let rows = count(&admin, &format!("SELECT count(*) FROM {target}")).await?;
            assert_eq!(rows, 0, "{target} after a refused reservation at {table}");
        }
        assert!(writer.calls().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn publication_finalization_failure_leaves_durable_pending_reservation_for_restart()
-> TestResult {
    for (table, condition) in [
        ("jobniac.generated_assets", "NEW.lifecycle = 'ACTIVE'"),
        ("jobniac.generation_intents", "NEW.state = 'COMPLETED'"),
    ] {
        let Some(fixture) = fixture().await? else {
            return Ok(());
        };
        let clock = TestClock::new(start());
        let writer = Writer::new(None, None);
        let env = environment(fixture, clock.clone(), writer.clone()).await?;
        let admin = env.fixture.database.admin_client().await;
        let claim = env
            .service
            .reserve_classified_pdf(
                PDF,
                "rollback-publication.pdf",
                &descriptor(GenerationPurpose::StandalonePdf, None)?,
            )
            .await?;
        fail_at(&admin, table, "UPDATE", condition).await?;
        assert!(
            env.service.materialize_classified(&claim).await.is_err(),
            "{table}"
        );
        let after = state(&admin, claim.asset_id()).await?;
        assert_eq!(after.lifecycle, "RESERVED", "{table}");
        assert_eq!(after.intent_state.as_deref(), Some("PENDING"), "{table}");
        assert_eq!(after.status, None);
        assert_eq!(writer.calls(), vec![claim.asset_id()]);

        clock.advance(GENERATION_LEASE);
        let restart = restarted(&env.fixture, &clock);
        assert_eq!(
            restart
                .recover_expired_generations(env.root, RETRY_BATCH_SIZE)
                .await?,
            vec![claim.asset_id()]
        );
        assert_eq!(
            state(&admin, claim.asset_id())
                .await?
                .intent_state
                .as_deref(),
            Some("ABANDONED")
        );
    }
    Ok(())
}

/// Rows that a failed classified publication must leave exactly as they were: the reservation and
/// its intent, the cleanup queue and the document tables.
async fn ledger_counts(admin: &Client) -> Result<Vec<i64>, Box<dyn Error>> {
    let mut counts = Vec::new();
    for target in [
        "document.documents",
        "document.blobs",
        "jobniac.generated_assets",
        "jobniac.generation_intents",
        "jobniac.generated_asset_cleanups",
    ] {
        counts.push(count(admin, &format!("SELECT count(*) FROM {target}")).await?);
    }
    Ok(counts)
}

/// Guarantee pinned here: when a classified publication fails after the file write, the whole
/// finalization rolls back. The reservation, the intent with its previous lease (token and expiry),
/// the document rows and the cleanup queue are exactly as they were before the attempt, so a later
/// reader sees the previous lease. This test pins the observable result rather than the
/// transaction boundaries.
#[tokio::test]
async fn failed_publication_rolls_back_every_row_and_keeps_the_previous_lease() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(Some(PublicationFailureCategory::FileWriteFailed), None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let admin = env.fixture.database.admin_client().await;
    let claim = env
        .service
        .reserve_classified_pdf(
            PDF,
            "half-state.pdf",
            &descriptor(GenerationPurpose::ProfileResume, None)?,
        )
        .await?;
    let before = state(&admin, claim.asset_id()).await?;
    let counts_before = ledger_counts(&admin).await?;
    fail_at(&admin, "jobniac.generated_asset_cleanups", "INSERT", "TRUE").await?;

    assert!(env.service.materialize_classified(&claim).await.is_err());
    assert_eq!(
        writer.calls(),
        vec![claim.asset_id()],
        "the file was written before the rollback"
    );
    assert_eq!(state(&admin, claim.asset_id()).await?, before);
    assert_eq!(ledger_counts(&admin).await?, counts_before);
    assert_eq!(before.lease_token, claim.intent.lease_token);
    Ok(())
}

#[tokio::test]
async fn failed_retirement_queue_rolls_back_abandonment_and_asset_together() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let admin = env.fixture.database.admin_client().await;
    let claim = env
        .service
        .reserve_classified_pdf(
            PDF,
            "rollback-retirement.pdf",
            &descriptor(GenerationPurpose::ProfileResume, None)?,
        )
        .await?;
    env.service.materialize_classified(&claim).await?;
    fail_at(&admin, "jobniac.generated_asset_cleanups", "INSERT", "TRUE").await?;
    assert!(env.service.abandon_generation(&claim).await.is_err());
    let after = state(&admin, claim.asset_id()).await?;
    assert_eq!(after.lifecycle, "ACTIVE");
    assert_eq!(after.intent_state.as_deref(), Some("PENDING"));
    assert_eq!(after.status, None);
    Ok(())
}

#[tokio::test]
async fn referenced_expired_pending_does_not_starve_later_eligible_limit_one() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let admin = env.fixture.database.admin_client().await;
    let retained = env
        .service
        .reserve_classified_pdf(
            PDF,
            "referenced.pdf",
            &descriptor(GenerationPurpose::ProfileResume, None)?,
        )
        .await?;
    env.service.materialize_classified(&retained).await?;
    let profile = Uuid::new_v4();
    admin
        .execute(
            "INSERT INTO profile.profiles (id, full_name, email, created_at, updated_at) \
             VALUES ($1, 'Synthetic retention owner', 'retention@example.test', $2, $2)",
            &[&profile, &start()],
        )
        .await?;
    admin
        .execute(
            "INSERT INTO profile.profile_personal_details \
             (profile_id, photo_document_id, created_at, updated_at) VALUES ($1, $2, $3, $3)",
            &[&profile, &retained.reservation.document_id, &start()],
        )
        .await?;
    clock.advance(Duration::seconds(1));
    let eligible = env
        .service
        .reserve_classified_pdf(
            PDF,
            "eligible.pdf",
            &descriptor(GenerationPurpose::ProfileResume, None)?,
        )
        .await?;
    clock.advance(GENERATION_LEASE);
    let repository = env.repository.clone();
    assert_eq!(
        repository.recover_expired_generations(env.root, 1).await?,
        vec![eligible.asset_id()]
    );
    assert!(
        repository
            .recover_expired_generations(env.root, 1)
            .await?
            .is_empty()
    );
    let kept = state(&admin, retained.asset_id()).await?;
    assert_eq!(kept.lifecycle, "ACTIVE");
    assert_eq!(kept.intent_state.as_deref(), Some("PENDING"));
    assert_eq!(kept.status, None);
    let retired = state(&admin, eligible.asset_id()).await?;
    assert_eq!(retired.lifecycle, "RETIRING");
    assert_eq!(retired.intent_state.as_deref(), Some("ABANDONED"));
    Ok(())
}

#[tokio::test]
async fn lost_reservation_ack_reconciles_original_operation_and_fence() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let inner = Arc::new(PgGeneratedAssetRepository::new(
        fixture.uow.clone(),
        clock.clone(),
    ));
    let lost = Arc::new(LostAcknowledgment {
        inner: inner.clone(),
        operation: "reserve",
        armed: AtomicBool::new(true),
    });
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let service = GeneratedAssetService::new(
        lost.clone() as Arc<dyn GeneratedAssetRepository>,
        writer.clone() as Arc<dyn GeneratedAssetWriter>,
        env.root,
        clock.clone(),
        true,
    );
    let claim = service
        .reserve_classified_pdf(
            PDF,
            "lost-reserve.pdf",
            &descriptor(GenerationPurpose::StandalonePdf, None)?,
        )
        .await?;
    assert_eq!(
        GeneratedAssetRepository::find_classified_reservation(inner.as_ref(), claim.asset_id())
            .await?,
        Some(claim.clone())
    );
    assert_eq!(
        service.materialize_classified(&claim).await?.outcome,
        MaterializationOutcome::Active
    );
    Ok(())
}

#[tokio::test]
async fn lost_standalone_activation_ack_preserves_durable_retention_without_synthetic_success()
-> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let inner = Arc::new(PgGeneratedAssetRepository::new(
        fixture.uow.clone(),
        clock.clone(),
    ));
    let lost = Arc::new(LostAcknowledgment {
        inner: inner.clone(),
        operation: "materialize",
        armed: AtomicBool::new(true),
    });
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let admin = env.fixture.database.admin_client().await;
    let service = GeneratedAssetService::new(
        lost as Arc<dyn GeneratedAssetRepository>,
        writer.clone() as Arc<dyn GeneratedAssetWriter>,
        env.root,
        clock.clone(),
        true,
    );
    let claim = env
        .service
        .reserve_classified_pdf(
            PDF,
            "lost-complete.pdf",
            &descriptor(GenerationPurpose::StandalonePdf, None)?,
        )
        .await?;
    assert!(service.materialize_classified(&claim).await.is_err());
    assert_eq!(
        state(&admin, claim.asset_id())
            .await?
            .intent_state
            .as_deref(),
        Some("COMPLETED")
    );
    assert_eq!(
        env.service.abandon_generation(&claim).await?,
        PreparationOutcome::Referenced
    );
    clock.advance(Duration::days(60));
    assert!(env.service.recover_expired_generations().await?.is_empty());
    assert_eq!(state(&admin, claim.asset_id()).await?.lifecycle, "ACTIVE");
    Ok(())
}

#[tokio::test]
async fn lease_that_expires_while_waiting_for_the_locks_refuses_before_any_write() -> TestResult {
    let Some(fixture) = fixture().await? else {
        return Ok(());
    };
    let clock = TestClock::new(start());
    let writer = Writer::new(None, None);
    let env = environment(fixture, clock.clone(), writer.clone()).await?;
    let claim = env
        .service
        .reserve_classified_pdf(
            PDF,
            "waiting.pdf",
            &descriptor(GenerationPurpose::ProfileResume, None)?,
        )
        .await?;
    let key = jobniac_infra::postgres::generated::bindings::document_lock_key_input(
        claim.reservation.document_id,
    );

    // Another session holds the document lock, so the materialization waits inside its transaction.
    let admin = env.fixture.database.admin_client().await;
    admin.batch_execute("BEGIN").await?;
    admin
        .execute(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            &[&key],
        )
        .await?;
    let repository = env.repository.clone();
    let task_writer = writer.clone();
    let task_claim = claim.clone();
    let waiting = tokio::spawn(async move {
        repository
            .materialize_classified(&task_claim, task_writer.as_ref())
            .await
    });
    let observer = env.fixture.database.admin_client().await;
    loop {
        let blocked: i64 = observer
            .query_one(
                "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted",
                &[],
            )
            .await?
            .get(0);
        if blocked > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // The lease runs out during the wait; the instant must be read after the lock is granted.
    clock.advance(GENERATION_LEASE + Duration::seconds(1));
    admin.batch_execute("COMMIT").await?;
    let outcome = waiting.await??;
    assert_eq!(outcome, MaterializationOutcome::Ineligible);
    assert!(
        writer.calls().is_empty(),
        "no file is written for an expired lease"
    );
    Ok(())
}
