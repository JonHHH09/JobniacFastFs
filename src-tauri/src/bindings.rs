//! Regenerates the TypeScript IPC bindings at `src/ipc/bindings.ts`. Running this test rewrites
//! the committed file; CI runs it and fails when `git diff` shows a change.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use specta_typescript::Typescript;

    #[test]
    fn export_typescript_bindings() -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/ipc/bindings.ts");
        crate::ipc_builder().export(Typescript::default(), path)?;
        Ok(())
    }

    /// A command that takes a raw `i64`, used only to prove the exporter's guard below.
    #[tauri::command]
    #[specta::specta]
    fn raw_i64_command(revision: i64) -> i64 {
        revision
    }

    /// `i64` values cross IPC as strings. The pinned exporter refuses to export any
    /// 64-bit integer (`BigIntForbidden`) instead of typing it as `number`, so a DTO field that
    /// stays a raw `i64` fails the export, and with it `export_typescript_bindings`.
    #[test]
    fn a_raw_i64_field_is_refused_by_the_exporter_instead_of_typed_as_number() {
        let path = std::env::temp_dir().join("jobniac-raw-i64-bindings.ts");
        let builder = tauri_specta::Builder::<tauri::Wry>::new()
            .commands(tauri_specta::collect_commands![raw_i64_command]);
        let failure = builder
            .export(Typescript::default(), &path)
            .expect_err("a raw i64 must not be exported");
        assert!(
            failure.to_string().contains("BigInt"),
            "unexpected export failure: {failure}"
        );
        // Best-effort cleanup: the export fails before writing, so the file normally is absent.
        drop(std::fs::remove_file(&path));
    }

    /// The committed bindings type no 64-bit integer as `number` or `bigint` by accident:
    /// the generated text carries no `bigint`.
    #[test]
    fn the_generated_bindings_use_no_bigint() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join("jobniac-bindings-bigint-check.ts");
        crate::ipc_builder().export(Typescript::default(), &path)?;
        let text = std::fs::read_to_string(&path)?;
        drop(std::fs::remove_file(&path));
        assert!(!text.contains("bigint"), "bindings contain a bigint type");
        Ok(())
    }
}
