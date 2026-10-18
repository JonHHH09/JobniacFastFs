//! The packaging smoke quit hook (`JOBNIAC_SMOKE_QUIT_AFTER_LOAD`).
//!
//! The hook is read once from the environment snapshot and is inert unless the value is exactly
//! `1`. When it is on, the desktop shell exits after its first successful load: the main window's
//! page load finished and the health use case returned a report. The process then exits with
//! status 0. Two outcomes exit with status 1 and a fixed message: the page did not finish loading
//! within [`SMOKE_TIMEOUT`], or the health use case was not available. A DOWN report still counts as
//! an answer, because the smoke checks the UI, not the database.
//!
//! The decision is one-shot: [`SmokeHook::decide`] yields the exit status for the first outcome only,
//! so a late timer cannot override a clean exit.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use jobniac_core::env::Environment;

/// The environment variable of the packaging smoke hook.
pub const SMOKE_QUIT_VARIABLE: &str = "JOBNIAC_SMOKE_QUIT_AFTER_LOAD";

/// How long the smoke run waits for the first page load and health answer before it fails.
pub const SMOKE_TIMEOUT: Duration = Duration::from_secs(60);

/// Exit status of a smoke run that loaded and answered.
pub const SMOKE_EXIT_OK: i32 = 0;
/// Exit status of a smoke run that failed.
pub const SMOKE_EXIT_FAILED: i32 = 1;

/// Fixed stderr text of the timeout outcome. It names no path or setting.
pub const SMOKE_TIMED_OUT_MESSAGE: &str = "smoke: the application did not load in time";
/// Fixed stderr text of the health outcome when the health use case is unavailable.
pub const SMOKE_HEALTH_REFUSED_MESSAGE: &str = "smoke: the health check did not answer";

/// Whether the hook is on: the exact value `1`, as for every feature flag.
#[must_use]
pub fn enabled(environment: &Environment) -> bool {
    environment.flag_enabled(SMOKE_QUIT_VARIABLE)
}

/// The outcome of a smoke run that decides the exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmokeOutcome {
    /// The page loaded and the health use case returned a report (any status).
    HealthAnswered,
    /// The health use case was not available, for example after a startup failure.
    HealthRefused,
    /// The page did not finish loading within [`SMOKE_TIMEOUT`].
    TimedOut,
}

impl SmokeOutcome {
    /// The process exit status of this outcome.
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::HealthAnswered => SMOKE_EXIT_OK,
            Self::HealthRefused | Self::TimedOut => SMOKE_EXIT_FAILED,
        }
    }

    /// The fixed stderr text printed before a failing exit, if any.
    #[must_use]
    pub const fn message(self) -> Option<&'static str> {
        match self {
            Self::HealthAnswered => None,
            Self::HealthRefused => Some(SMOKE_HEALTH_REFUSED_MESSAGE),
            Self::TimedOut => Some(SMOKE_TIMED_OUT_MESSAGE),
        }
    }
}

/// The one-shot state of a smoke run, shared by the page-load hook and the timer.
#[derive(Debug, Default)]
pub struct SmokeHook {
    started: AtomicBool,
    decided: AtomicBool,
}

impl SmokeHook {
    /// A hook that has not started and has not decided.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True on the first call only: the caller then starts the health check. Later page loads, such
    /// as a reload, do not start another one.
    pub fn start(&self) -> bool {
        !self.started.swap(true, Ordering::AcqRel)
    }

    /// The exit status for the first outcome only; every later outcome yields `None`.
    pub fn decide(&self, outcome: SmokeOutcome) -> Option<i32> {
        (!self.decided.swap(true, Ordering::AcqRel)).then(|| outcome.exit_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hook_is_on_only_for_the_exact_value_one() {
        assert!(enabled(&Environment::from_pairs([(
            SMOKE_QUIT_VARIABLE,
            "1"
        )])));
        for value in ["", "0", "true", "01", " 1", "1 "] {
            let environment = Environment::from_pairs([(SMOKE_QUIT_VARIABLE, value)]);
            assert!(!enabled(&environment), "{value:?} enabled the hook");
        }
        assert!(!enabled(&Environment::default()));
    }

    #[test]
    fn an_invalid_unicode_value_is_off() {
        let environment = Environment::default().with_invalid(SMOKE_QUIT_VARIABLE);
        assert!(!enabled(&environment));
    }

    #[test]
    fn a_health_answer_exits_zero_and_a_refusal_or_timeout_exits_one() {
        assert_eq!(SmokeOutcome::HealthAnswered.exit_code(), 0);
        assert_eq!(SmokeOutcome::HealthRefused.exit_code(), 1);
        assert_eq!(SmokeOutcome::TimedOut.exit_code(), 1);
        assert_eq!(SmokeOutcome::HealthAnswered.message(), None);
        assert_eq!(
            SmokeOutcome::TimedOut.message(),
            Some(SMOKE_TIMED_OUT_MESSAGE)
        );
    }

    #[test]
    fn the_messages_name_no_path_or_setting() {
        for text in [SMOKE_TIMED_OUT_MESSAGE, SMOKE_HEALTH_REFUSED_MESSAGE] {
            assert!(!text.contains('/') && !text.contains('\\'), "{text}");
            assert!(!text.contains("JOBNIAC"), "{text}");
        }
    }

    #[test]
    fn the_page_load_starts_the_health_check_once() {
        let hook = SmokeHook::new();
        assert!(hook.start());
        assert!(!hook.start());
        assert!(!hook.start());
    }

    #[test]
    fn only_the_first_outcome_decides_the_exit_status() {
        let hook = SmokeHook::new();
        assert_eq!(hook.decide(SmokeOutcome::HealthAnswered), Some(0));
        assert_eq!(hook.decide(SmokeOutcome::TimedOut), None);
        assert_eq!(hook.decide(SmokeOutcome::HealthRefused), None);
    }

    #[test]
    fn a_timeout_first_fails_and_a_late_answer_cannot_turn_it_into_success() {
        let hook = SmokeHook::new();
        assert_eq!(hook.decide(SmokeOutcome::TimedOut), Some(1));
        assert_eq!(hook.decide(SmokeOutcome::HealthAnswered), None);
    }
}
