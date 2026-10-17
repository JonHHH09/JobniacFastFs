// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::ffi::OsStr;
use std::process::ExitCode;

use jobniac_runtime::worker;

fn main() -> ExitCode {
    // The sandboxed PDF worker is this same executable re-run with the hidden `__worker` mode, so
    // it is recognized before any window or plugin starts.
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if worker::selects_worker_mode(arguments.first().map(AsRef::as_ref)) {
        let rest: Vec<&OsStr> = arguments[1..].iter().map(AsRef::as_ref).collect();
        return worker::run(&rest);
    }
    jobniac_desktop_lib::run();
    ExitCode::SUCCESS
}
