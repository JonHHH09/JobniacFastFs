//! The shell commands of the window close gate: the job and profile
//! dirty reports, and the request that closes the window after the profile discard is confirmed.
//!
//! Rust decides every close. React only reports its drafts through `shell_set_dirty`, and asks for a
//! close after the user chose to discard; the gate then runs again in the `CloseRequested` handler.

use tauri::{State, Window};

use crate::error::CommandError;
use crate::shell_close_dto::ShellDirtyWorkspace;
use crate::state::AppState;

/// Records whether a job or profile draft holds unsaved edits. React sends it only when the flag
/// changes. It is not guarded by a single-flight slot: it reports React's draft and touches no rows.
#[tauri::command]
#[specta::specta]
pub fn shell_set_dirty(
    state: State<'_, AppState>,
    workspace: ShellDirtyWorkspace,
    dirty: bool,
) -> Result<(), CommandError> {
    run_set_dirty(&state, workspace, dirty);
    Ok(())
}

fn run_set_dirty(state: &AppState, workspace: ShellDirtyWorkspace, dirty: bool) {
    match workspace {
        ShellDirtyWorkspace::Job => state.set_job_draft_dirty(dirty),
        ShellDirtyWorkspace::Profile => state.set_profile_draft_dirty(dirty),
    }
}

/// Asks for the window to close after the user discarded the profile edits in the confirmation.
/// The request marks one confirmed close and asks the window to close; the `CloseRequested` handler
/// runs the whole gate once more, and the mark bypasses only the profile confirmation.
#[tauri::command]
#[specta::specta]
pub fn shell_request_close(window: Window, state: State<'_, AppState>) -> Result<(), CommandError> {
    state.request_confirmed_close();
    if window.close().is_err() {
        // No close was requested, so the mark must not outlive this call; the window stays open.
        state.take_confirmed_close();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_set_dirty;
    use crate::shell_close_dto::ShellDirtyWorkspace;
    use crate::state::{AppState, CloseRefusal, JOB_UNSAVED_CLOSE, PROFILE_CONFIRM_CLOSE};
    use jobniac_core::env::Environment;
    use jobniac_runtime::services::compose;

    fn state() -> AppState {
        AppState::new(compose(&Environment::default()))
    }

    #[test]
    fn a_dirty_job_report_refuses_the_close_until_it_is_cleared() {
        let state = state();
        run_set_dirty(&state, ShellDirtyWorkspace::Job, true);
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "job",
                reason: "refuse_unsaved",
                message_id: JOB_UNSAVED_CLOSE,
            })
        );
        run_set_dirty(&state, ShellDirtyWorkspace::Job, false);
        assert_eq!(state.close_refusal(), None);
    }

    #[test]
    fn a_dirty_profile_report_asks_for_the_discard_confirmation_until_it_is_cleared() {
        let state = state();
        run_set_dirty(&state, ShellDirtyWorkspace::Profile, true);
        assert_eq!(
            state.close_refusal(),
            Some(CloseRefusal {
                workspace: "profile",
                reason: "confirm_discard",
                message_id: PROFILE_CONFIRM_CLOSE,
            })
        );
        run_set_dirty(&state, ShellDirtyWorkspace::Profile, false);
        assert_eq!(state.close_refusal(), None);
    }

    #[test]
    fn a_clean_profile_report_leaves_the_job_refusal_standing() {
        let state = state();
        run_set_dirty(&state, ShellDirtyWorkspace::Job, true);
        run_set_dirty(&state, ShellDirtyWorkspace::Profile, true);
        run_set_dirty(&state, ShellDirtyWorkspace::Profile, false);
        assert_eq!(
            state.close_refusal().map(|refusal| refusal.workspace),
            Some("job")
        );
    }
}
