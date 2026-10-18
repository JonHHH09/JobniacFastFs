//! The German source reads (`SqliteGermanSources`) and the generic profile graph port composed with
//! the concrete SQLite profile repository, over a migrated temporary SQLite file. All data is
//! synthetic.

mod sqlite_german_support;
mod sqlite_support;

use jobniac_core::generation::generic::ProfileGraphPort;
use jobniac_core::german::generation::{JobAggregatePort, PersonalDetailsPort};
use jobniac_infra::sqlite::generated::ports::SqliteProfileGraphs;
use jobniac_infra::sqlite::german::SqliteGermanSources;
use jobniac_infra::sqlite::profiles::SqliteProfileRepository;
use sqlite_german_support::{TestResult, start};
use uuid::Uuid;

#[tokio::test]
async fn the_job_aggregate_is_read_and_a_missing_job_is_none() -> TestResult {
    let fixture = start()?;
    let sources = SqliteGermanSources::new(fixture.uow.clone());
    let found = sources
        .find_job(fixture.job)
        .await?
        .ok_or("the seeded job must be found")?;
    assert_eq!(found.job.id, fixture.job);
    assert_eq!(found.job.title, "Synthetic Job");
    assert!(found.text_ingestion.is_some());
    assert_eq!(sources.find_job(Uuid::new_v4()).await?, None);
    Ok(())
}

#[tokio::test]
async fn the_personal_details_envelope_is_read_and_a_missing_profile_is_none() -> TestResult {
    let fixture = start()?;
    let sources = SqliteGermanSources::new(fixture.uow.clone());
    let envelope = sources
        .find_personal_details(fixture.profile)
        .await?
        .ok_or("the seeded profile must be found")?;
    assert_eq!(envelope.profile_id, fixture.profile);
    assert_eq!(envelope.profile_revision, 0);
    assert!(envelope.personal_details.is_none());
    assert_eq!(sources.find_personal_details(Uuid::new_v4()).await?, None);
    Ok(())
}

#[tokio::test]
async fn the_profile_graph_port_reads_through_the_sqlite_profile_repository() -> TestResult {
    let fixture = start()?;
    let graphs = SqliteProfileGraphs::new(fixture.uow.clone(), SqliteProfileRepository);
    let graph = graphs
        .find_profile(fixture.profile)
        .await?
        .ok_or("the seeded profile must be found")?;
    assert_eq!(graph.profile.id, fixture.profile);
    assert_eq!(graphs.find_profile(Uuid::new_v4()).await?, None);
    Ok(())
}
