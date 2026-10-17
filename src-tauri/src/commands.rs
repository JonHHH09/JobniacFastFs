//! Tauri command adapters. Each one calls a core use case and maps the result to a DTO.

use tauri::State;

use crate::dto::HealthReportDto;
use crate::error::CommandError;
use crate::state::AppState;

/// Run the shared health check (the same use case the MCP health tool calls).
#[tauri::command]
#[specta::specta]
pub async fn health_check(state: State<'_, AppState>) -> Result<HealthReportDto, CommandError> {
    run_health_check(&state).await
}

async fn run_health_check(state: &AppState) -> Result<HealthReportDto, CommandError> {
    let services = state.services()?;
    let _slot = state.begin_health_check()?;
    let report = services.health.check().await;
    Ok(HealthReportDto::from(&report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HealthStatusDto;
    use jobniac_core::env::Environment;
    use jobniac_runtime::services::compose;

    #[tokio::test]
    async fn empty_configuration_reports_down_without_connecting() -> Result<(), CommandError> {
        let services = compose(&Environment::default());
        let state = AppState::new(services);
        let dto = run_health_check(&state).await?;
        assert_eq!(dto.status, HealthStatusDto::Down);
        assert_eq!(dto.metadata.total_checks, 1);
        assert_eq!(dto.metadata.failed_checks, 1);
        Ok(())
    }

    #[tokio::test]
    async fn a_second_check_while_one_runs_is_refused_as_busy() -> Result<(), CommandError> {
        let services = compose(&Environment::default());
        let state = AppState::new(services);
        let _held = state.begin_health_check()?;
        assert_eq!(
            run_health_check(&state).await.err(),
            Some(CommandError::busy("health.busy"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn startup_failure_is_returned_as_command_error() {
        let state = AppState::new(Err(jobniac_runtime::services::StartupError));
        assert_eq!(
            run_health_check(&state).await.err(),
            Some(CommandError::startup_failed())
        );
    }
}
