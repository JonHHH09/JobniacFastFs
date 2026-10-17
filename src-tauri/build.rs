fn main() {
    tauri_build::build();

    // Tauri imports `TaskDialogIndirect` from comctl32, which exists only in Common Controls v6.
    // The application binary gets v6 from the manifest Tauri embeds, but the unit-test harness
    // does not. Without the dependency it loads v5 and fails with STATUS_ENTRYPOINT_NOT_FOUND.
    // `/MANIFESTDEPENDENCY` is an MSVC linker option.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
