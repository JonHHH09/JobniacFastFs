//! Tauri desktop shell for JobniacFast. Commands are thin adapters over core use cases.
#![forbid(unsafe_code)]

mod arbeitnow_commands;
mod arbeitnow_dto;
mod artifact_commands;
mod artifact_dto;
mod bindings;
mod commands;
#[cfg(test)]
mod config_tests;
mod delete_commands;
mod delete_dto;
mod document_commands;
mod document_dto;
mod dto;
mod error;
mod generation_commands;
mod generation_dto;
mod job_commands;
mod job_dto;
mod job_link_commands;
mod job_link_dto;
mod match_commands;
mod match_dto;
mod page_image;
mod profile_commands;
mod profile_dto;
mod profile_pdf_commands;
mod profile_pdf_dto;
mod settings_commands;
mod settings_dto;
mod shell_close_commands;
mod shell_close_dto;
mod state;

pub use dto::{
    CleanupReportDto, CleanupStatusDto, ErrorCategoryDto, HealthReportDto, HealthStatusDto,
};
pub use error::CommandError;
pub use state::AppState;

use std::path::PathBuf;
use std::sync::Arc;

use jobniac_runtime::app_settings::AppSettings;
use jobniac_runtime::mcp_server::{McpSupervisor, ProcessSpawner};
use jobniac_runtime::pdfium_location::resolve_pdfium_dir;
use jobniac_runtime::services::compose_with_pdfium;
use jobniac_runtime::smoke::{self, SMOKE_TIMEOUT, SmokeHook, SmokeOutcome};
use tauri::{Emitter, Manager, WindowEvent};
#[cfg(windows)]
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_specta::{Builder, collect_commands};

/// The typed IPC surface: every command is registered with the invoke handler and exported to
/// TypeScript by `tests/bindings.rs`.
pub fn ipc_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
        commands::health_check,
        profile_commands::list_profiles,
        profile_commands::get_profile,
        profile_commands::create_profile,
        profile_commands::update_profile,
        profile_commands::update_project,
        profile_commands::search_profiles,
        profile_commands::get_personal_details,
        profile_commands::update_personal_details,
        job_commands::add_job_from_text,
        job_commands::list_jobs,
        job_commands::get_job,
        job_commands::search_jobs,
        job_commands::update_job,
        job_link_commands::job_link_capabilities_get,
        job_link_commands::job_add_link,
        arbeitnow_commands::arbeitnow_capabilities_get,
        arbeitnow_commands::arbeitnow_scan,
        arbeitnow_commands::arbeitnow_next_page,
        arbeitnow_commands::arbeitnow_import_row,
        match_commands::match_capabilities_get,
        match_commands::match_analyze_pair,
        match_commands::match_analyze_page,
        match_commands::match_list_reports,
        match_commands::match_get_report,
        match_commands::review_set_draft_dirty,
        match_commands::review_submit,
        match_commands::review_get,
        match_commands::review_list,
        match_commands::review_list_disagreements,
        match_commands::review_acknowledge_disagreement,
        match_commands::review_link_disagreement,
        document_commands::documents_capabilities_get,
        document_commands::documents_list,
        document_commands::document_reset,
        document_commands::document_load_metadata,
        document_commands::document_preview_stored,
        document_commands::document_render_page,
        document_commands::document_choose_file,
        document_commands::document_import_selected,
        document_commands::document_preview_selected,
        artifact_commands::artifact_capabilities_get,
        artifact_commands::artifact_preview_document,
        artifact_commands::artifact_show_page,
        artifact_commands::artifact_export_document,
        generation_commands::generation_capabilities_get,
        generation_commands::generation_standalone,
        generation_commands::generation_resume,
        delete_commands::deletion_capabilities_get,
        delete_commands::request_profile_delete,
        delete_commands::delete_profile,
        delete_commands::request_job_delete,
        delete_commands::delete_job,
        profile_pdf_commands::profile_pdf_capabilities_get,
        profile_pdf_commands::profile_pdf_ingest,
        profile_pdf_commands::profile_pdf_confirm_replacement,
        profile_pdf_commands::profile_pdf_cancel_replacement,
        profile_pdf_commands::profile_pdf_lookup_source,
        shell_close_commands::shell_set_dirty,
        shell_close_commands::shell_request_close,
        settings_commands::settings_get,
        settings_commands::mcp_server_set_enabled,
        settings_commands::mcp_server_set_port,
        settings_commands::mcp_server_copy_connection,
    ])
}

/// Answers a request to the page-image scheme from the resident raster. The request is
/// refused by [`page_image::serve`] when it does not name the current handle and revision; a missing
/// state refuses every request as not found.
fn serve_page_image(
    context: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let response = match (
        context.app_handle().try_state::<AppState>(),
        page_image::PageViewer::of_request(&request),
    ) {
        (Some(state), page_image::PageViewer::Documents) => {
            page_image::serve(&state.page_rasters(), &request)
        }
        (Some(state), page_image::PageViewer::Artifacts) => {
            page_image::serve(&state.artifact_rasters(), &request)
        }
        (None, _) => page_image::serve(&page_image::PageRasterStore::default(), &request),
    };
    responder.respond(response);
}

/// The window close gate. Rust decides whether the window may
/// close: a refusing workspace prevents the close and React is told which one refused, so it can
/// show the copy of the message id. A window without the state is not gated.
fn close_gate(window: &tauri::Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    let Some(state) = window.try_state::<AppState>() else {
        return;
    };
    // The confirmed-close mark is taken by every gate run, so it covers exactly one close attempt.
    let confirmed_discard = state.take_confirmed_close();
    if let Some(refusal) = state.close_refusal_with(confirmed_discard) {
        api.prevent_close();
        // The event only informs the screen; a failed emit must not let the close through.
        let _ = window.emit(state::CLOSE_REFUSED_EVENT, refusal);
        return;
    }
    // The close goes ahead: the page rasters and the held Arbeitnow rows do not outlive the window.
    state.release_document_work();
    state.clear_artifact_view();
    state.clear_arbeitnow_session();
    // The supervised MCP server does not outlive the window.
    state.mcp_supervisor().close();
}

/// The label of the one window of the desktop shell (`app.windows` in `tauri.conf.json`).
const MAIN_WINDOW_LABEL: &str = "main";

/// Moves a smoke outcome to the process exit: the first decisive outcome prints its fixed message
/// (when it fails) and exits with its status. Later outcomes are ignored.
fn finish_smoke(app: &tauri::AppHandle<tauri::Wry>, hook: &SmokeHook, outcome: SmokeOutcome) {
    if let Some(code) = hook.decide(outcome) {
        if let Some(message) = outcome.message() {
            eprintln!("{message}");
        }
        app.exit(code);
    }
}

/// Starts the smoke timer: when no outcome was decided within [`SMOKE_TIMEOUT`], the run fails.
fn arm_smoke_timeout(app: tauri::AppHandle<tauri::Wry>, hook: Arc<SmokeHook>) {
    std::thread::spawn(move || {
        std::thread::sleep(SMOKE_TIMEOUT);
        finish_smoke(&app, &hook, SmokeOutcome::TimedOut);
    });
}

/// The title of the native message that reports a refused startup.
const STARTUP_REFUSED_TITLE: &str = "Jobniac";

/// Ends a startup the configuration refused. `message` is one of the fixed startup texts; it never
/// names a setting or a value. It goes to stderr for a console run and into a native message box,
/// because a Windows release build has no console and would otherwise exit with no sign at all.
/// No window exists yet, so the dialog has no parent. The dialog is Windows-only: the plugin's
/// blocking call must not run on the main thread, and there it dispatches to the native message
/// box on its own thread; on Linux the loop is not running yet and the call could hang, and a
/// Linux launch always has stderr. The exit does not depend on the dialog result.
fn exit_startup_refused(app: &tauri::AppHandle<tauri::Wry>, message: &str) -> ! {
    eprintln!("{message}");
    #[cfg(windows)]
    {
        let _ = app
            .dialog()
            .message(message)
            .title(STARTUP_REFUSED_TITLE)
            .kind(MessageDialogKind::Error)
            .blocking_show();
    }
    #[cfg(not(windows))]
    let _ = (app, STARTUP_REFUSED_TITLE);
    std::process::exit(1);
}

/// Start the desktop application. The environment is read once. The Tauri application is built
/// first, because the PDFium library lives in its resource directory; the services are composed
/// before any window is shown, so a failed composition exits with status 1 before a window exists
/// (the CLI composes before `run_desktop` for the same reason). Windows are
/// created when the event loop starts, after the state is managed.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[allow(clippy::expect_used)]
pub fn run() {
    let environment = jobniac_runtime::process_environment();
    // The packaging smoke quit hook: on only for the exact value `1`.
    let smoke = smoke::enabled(&environment).then(|| Arc::new(SmokeHook::new()));
    let builder = ipc_builder();
    let app = tauri::Builder::default()
        // The dialog is used from Rust only (the picker seam); no capability grants it to the webview.
        .plugin(tauri_plugin_dialog::init())
        // The clipboard is written from Rust only (the MCP connection copy); no capability grants it
        // to the webview, so the token never crosses IPC.
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(builder.invoke_handler())
        .register_asynchronous_uri_scheme_protocol(page_image::PAGE_SCHEME, serve_page_image)
        .on_window_event(close_gate)
        .on_page_load({
            let smoke = smoke.clone();
            move |webview, payload| {
                // A page that starts loading holds no draft: what the previous page reported as
                // dirty is stale after a reload (F5, a dev full reload). This runs before the
                // smoke early return, which only gates the packaging smoke hook.
                if webview.label() == MAIN_WINDOW_LABEL
                    && matches!(payload.event(), tauri::webview::PageLoadEvent::Started)
                    && let Some(state) = webview.app_handle().try_state::<AppState>()
                {
                    state.reset_webview_reports();
                }
                let Some(hook) = smoke.as_ref() else {
                    return;
                };
                if webview.label() != MAIN_WINDOW_LABEL
                    || !matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
                    || !hook.start()
                {
                    return;
                }
                let app = webview.app_handle().clone();
                let health = app.try_state::<AppState>().and_then(|state| {
                    state
                        .services()
                        .ok()
                        .map(|services| Arc::clone(&services.health))
                });
                let hook = Arc::clone(hook);
                tauri::async_runtime::spawn(async move {
                    let outcome = match health {
                        // The report is an answer whatever its status (see `jobniac_runtime::smoke`).
                        Some(health) => {
                            let _report = health.check().await;
                            SmokeOutcome::HealthAnswered
                        }
                        None => SmokeOutcome::HealthRefused,
                    };
                    finish_smoke(&app, &hook, outcome);
                });
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Diagnostics first, so the startup warnings below are not dropped: stderr, plus the per-user log
    // file when `JOBNIAC_LOG_FILE` is exactly `1`. The panic hook leaves one fixed line, because a
    // release build aborts on panic under the Windows GUI subsystem.
    let log_file = jobniac_runtime::diagnostics::log_file_path(
        &environment,
        app.path().app_log_dir().ok().as_deref(),
    );
    jobniac_runtime::diagnostics::init_tracing(log_file.as_deref());
    jobniac_runtime::diagnostics::install_panic_hook(log_file);

    // The PDFium library comes from the bundle: the `pdfium` folder of the Tauri resource directory
    // (the install folder on Windows, `/usr/lib/JobniacFast` on Linux packages). `JOBNIAC_PDFIUM_DIR`
    // still wins when set. A missing library leaves the PDF operations failing with their own fixed
    // messages, as before; startup does not change, but one fixed warning says why.
    let bundle_roots: Vec<PathBuf> = app.path().resource_dir().into_iter().collect();
    let pdfium_dir = jobniac_runtime::pdfium_location::report_pdfium_dir(resolve_pdfium_dir(
        &environment,
        &bundle_roots,
        |path| path.is_file(),
    ));
    let services = match compose_with_pdfium(&environment, pdfium_dir) {
        Ok(services) => services,
        // The message is the fixed startup text; it never names a setting or a value.
        Err(error) => exit_startup_refused(app.handle(), &error.to_string()),
    };
    // The opt-in link capture: composed only with the exact flag and a database. A
    // composition error stops startup like the services above.
    let job_links = match jobniac_runtime::job_links::compose_job_links(
        &environment,
        services.database.as_ref(),
    ) {
        Ok(job_links) => job_links,
        Err(error) => exit_startup_refused(app.handle(), &error.to_string()),
    };
    // The opt-in Arbeitnow services: composed only with the exact flag, valid keys and a database.
    let arbeitnow = match jobniac_runtime::arbeitnow::compose_arbeitnow(
        &environment,
        services.database.as_ref(),
    ) {
        Ok(arbeitnow) => arbeitnow,
        Err(error) => exit_startup_refused(app.handle(), &error.to_string()),
    };
    // The settings file lives in the app config directory. A missing or broken file means the
    // defaults (the MCP server off); it never blocks startup.
    let config_dir = app.path().app_config_dir().ok();
    let settings = config_dir
        .as_deref()
        .map(AppSettings::load)
        .unwrap_or_default();
    // The sidecar `jobniac-cli` sits next to this executable; the child gets a filtered copy of the
    // app's environment (see `jobniac_runtime::mcp_server::child_environment`).
    let mcp = Arc::new(McpSupervisor::new(
        Arc::new(ProcessSpawner::beside_current_exe()),
        std::env::vars_os().collect(),
    ));
    app.manage(
        AppState::new(Ok(services))
            .with_job_links(job_links)
            .with_arbeitnow(arbeitnow)
            .with_settings(config_dir, settings)
            .with_mcp_supervisor(Arc::clone(&mcp)),
    );
    if settings.mcp.enabled {
        // The start waits for the listener (up to 15 s), so it runs off the main thread. A failure
        // leaves the status `failed`; the app starts regardless.
        let port = settings.mcp.port;
        std::thread::spawn(move || {
            mcp.start(port);
        });
    }
    if let Some(hook) = &smoke {
        arm_smoke_timeout(app.handle().clone(), Arc::clone(hook));
    }
    app.run(|app, event| {
        // Shutdown cancels the PDF worker calls still in flight and drops the page raster.
        if let (tauri::RunEvent::Exit, Some(state)) = (event, app.try_state::<AppState>()) {
            state.release_document_work();
            state.clear_artifact_view();
            // The held Arbeitnow rows and candidate tokens do not outlive the process.
            state.clear_arbeitnow_session();
            // The supervised MCP child is killed and reaped with the app.
            state.mcp_supervisor().close();
        }
    });
}
