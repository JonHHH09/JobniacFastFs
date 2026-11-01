# Clean-machine smoke checklist

Manual run of the install, use, MCP and uninstall smoke on a machine with nothing from Jobniac on
it. CI runs a scripted version on an ephemeral runner,
but the owner's physical or VM run is still required before publication. Run it on a Windows 11
virtual machine or in Windows Sandbox, not on your development machine.

Record each step as PASS or FAIL with the time and the output. Keep the record outside the
repository, and do not write machine names, user names or local paths into any tracked file.

## 0. Prepare the machine

1. Start from a clean snapshot of Windows 11 with no Jobniac and no development
   tools. Take a snapshot you can return to.
2. Put the installer and its `SHA256SUMS` file in a new folder outside the repository, for example
   `C:\smoke\in`. Use the artifacts from the release draft or the build workflow for the commit under test.
3. Confirm the baseline. Expected: no output.
   ```powershell
   Get-ChildItem $env:LOCALAPPDATA, $env:APPDATA -Filter '*obniac*' -Force -ErrorAction SilentlyContinue
   Get-ChildItem $env:LOCALAPPDATA -Filter 'io.github.jonhhh09.jobniacfast*' -Force -ErrorAction SilentlyContinue
   ```
   Failure: anything listed. Restore the snapshot; the run starts from a clean state.

## 1. Verify the installer

4. Compare the digest with `SHA256SUMS`. Expected: the two values are equal.
   ```powershell
   Get-FileHash -Algorithm SHA256 C:\smoke\in\JobniacFast_<version>_x64-setup.exe
   Select-String -Path C:\smoke\in\SHA256SUMS -Pattern 'x64-setup'
   ```
   Failure: different values. Stop; do not run the installer.

## 2. SmartScreen

5. Start the installer by double-clicking it. Expected: a SmartScreen dialog, "Windows protected
   your PC", because the build is unsigned.
   Click **More info**, then **Run anyway**.
   Failure: no **Run anyway** option (for example, a policy blocks the file). Stop and record it.

## 3. Install

6. Complete the installer. Expected: no administrator prompt (per-user install, the NSIS default),
   and the WebView2 step either finds the runtime already installed (Windows 11 normally has it) or
   runs the bootstrapper.
   Failure: a UAC prompt appears. The installer is not per-user; record it as a defect.
7. Check the install folder. Expected: `jobniac.exe`, `jobniac-cli.exe` and `uninstall.exe` in
   `%LOCALAPPDATA%\JobniacFast`. Confirm the folder name first (the installer's product name is
   `JobniacFast`):
   ```powershell
   Get-ChildItem "$env:LOCALAPPDATA\JobniacFast" | Select-Object Name
   ```
   Failure: no `jobniac-cli.exe`, although `bundle.externalBin` declares it in
   `src-tauri/tauri.release.conf.json` (the release config merged by `--config`). Stop the MCP steps and record it as a defect. Failure: the
   folder is elsewhere. Find it with
   `Get-ChildItem $env:LOCALAPPDATA, $env:ProgramFiles -Recurse -Filter jobniac*.exe -ErrorAction SilentlyContinue`
   and record the path in the run record.

## 4. CLI basics

8. Check the version. Expected: `Jobniac <version>`, exit code 0.
   ```powershell
   & "$env:LOCALAPPDATA\JobniacFast\jobniac-cli.exe" --version; $LASTEXITCODE
   ```
9. Check the help. Expected: a usage text on stdout listing `mcp`, `http`, `migrate`, `cleanup`,
   `setup-generated-root` and the opt-in flags, exit code 0.
   ```powershell
   & "$env:LOCALAPPDATA\JobniacFast\jobniac-cli.exe" help; $LASTEXITCODE
   ```

## 5. First launch

10. Start `jobniac.exe` from the Start menu. Expected: a window titled `JobniacFast` opens. The app creates
    the database file on its first start, so health reports `UP`. (If the file cannot be opened,
    health reports `DOWN` with `CONNECTION_UNAVAILABLE`.)
11. Close the window. Expected: it closes without a dialog, because nothing is unsaved.
    The automated smoke (`scripts/release/clean_machine_smoke.py`, launch step) uses the hook
    `JOBNIAC_SMOKE_QUIT_AFTER_LOAD=1`, which is implemented (`crates/runtime/src/smoke.rs`, wired in
    `src-tauri/src/lib.rs`). This manual step stays as the interactive check of the window and its
    close behaviour; it does not replace the hook.

## 6. Database and configuration

12. Note where the database file will be: `%LOCALAPPDATA%\Jobniac\data\jobniac.sqlite3`. It does not
    exist yet. Nothing has to be installed for it. To use another location, set `JOBNIAC_DB_PATH` to an
    absolute file path in a new PowerShell window (see
    [MCP client registration, section 1](mcp-client-registration.md#1-database-location)).
13. Relaunch `jobniac.exe` from that window. Expected: the window opens and creates the database file.
14. Migrate. Expected on a file the app has already created: `Schema is current (V0001); nothing to apply.`
    On a missing file (run this before step 13 to check): `Applied migrations: V0001`. Exit code 0 both
    times.
    ```powershell
    & "$env:LOCALAPPDATA\JobniacFast\jobniac-cli.exe" migrate; $LASTEXITCODE
    ```
    Failure: any `Jobniac migrate:` line. Record the text.
15. Check that the file exists. Expected: `True`.
    ```powershell
    Test-Path "$env:LOCALAPPDATA\Jobniac\data\jobniac.sqlite3"
    ```

## 7. Health

16. Check health. Expected: `UP`, `errorCategory` `NONE`. Use the MCP check in step 22 (the UI
    health display is secondary). Failure: `DOWN`. Read `errorCategory` and the settings.

## 8. Create data

17. In the app, create one profile and one job with synthetic content (no real CV, no real
    contact details). Expected: both appear in the lists.

## 9. Import and preview a PDF

18. Put a synthetic PDF (generated for the run or copied from the repository's test fixtures, never a
    real document) in a folder outside the repository, for example `C:\smoke\import`. Leave
    `JOBNIAC_DOCUMENT_IMPORT_ROOT` unset: the desktop app accepts the file picked in its dialog.
19. Pick the PDF from the document screen, import it and open the preview. Expected: the import completes,
    and the preview shows the first page.
    Failure: "file is not readable or contains a symbolic link" for a path that is valid. Check the
    path is absolute (and under the import root, when `JOBNIAC_DOCUMENT_IMPORT_ROOT` is set), and that
    no component ends in a dot or a space.

## 10. Generate a PDF (optional, creates the generated root)

20. Set the generated root for this run, in the same environment:
    `JOBNIAC_GENERATED_ENABLED=1`. Then create the root:
    ```powershell
    & "$env:LOCALAPPDATA\JobniacFast\jobniac-cli.exe" setup-generated-root; $LASTEXITCODE
    ```
    Expected: `Generated root <uuid>: <status>.`, exit code 0. Failure: `Jobniac setup-generated-root: ...`
    Record the text.
21. Restart the app, generate a resume PDF for the profile and job from step 17. Expected: a PDF is
    written under the generated root, and it opens in a PDF viewer.
    This step creates `%LOCALAPPDATA%\Jobniac\generated`. Decide before the run whether it is an
    allowed residue (section 12); the CI smoke does not create it.

## 11. MCP exercise

22. From an MCP client (or the JSON-RPC file in
    [mcp-client-registration.md, section 4](mcp-client-registration.md#4-verify-the-stdio-mode)),
    run `initialize`, then `tools/list`, then `tools/call` for `health`. Expected: 34 tools listed
    with no opt-in flags set, `health` returns `UP`, and stdout carries only JSON-RPC lines.
    ```powershell
    (Get-Content .\mcp-out.jsonl | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object id -eq 2).result.tools.Count
    ```
    Expected: `34`. Failure: any other number, a non-JSON line on stdout, or a missing response.
    Record the protocol version the client negotiated.

## 12. Close gate

23. Open a profile, change a field without saving, then click the window's close button.
    Expected: a dialog titled `Discard unsaved changes?` with the text `Discard your unsaved edits and close Jobniac?`.
    Click **Cancel**: the window stays open and the edit is still there. Close again and confirm:
    the app closes. Failure: the window closes with no dialog.

## 13. Uninstall and residue

24. Stop every process: `Get-Process -Name 'jobniac*' -ErrorAction SilentlyContinue` returns nothing,
    and no `jobniac-cli` process is left from the MCP steps.
25. Uninstall through Settings, Apps, JobniacFast, Uninstall. The silent form (`/S`, the CI
    variant) is a separate cycle: restore the snapshot taken after step 3 and run it there.
    ```powershell
    Start-Process -FilePath "$env:LOCALAPPDATA\JobniacFast\uninstall.exe" -ArgumentList '/S' -Wait
    ```
    Expected: the folder `%LOCALAPPDATA%\JobniacFast` is gone. If it still exists right after the
    process exits, wait a few seconds and check again, and record that it did.
26. Check the residue. Expected: only the allowed entries below.
    ```powershell
    Get-ChildItem $env:LOCALAPPDATA, $env:APPDATA -Filter '*obniac*' -Force -ErrorAction SilentlyContinue | Select-Object FullName
    Get-ChildItem $env:LOCALAPPDATA -Filter 'io.github.jonhhh09.jobniacfast*' -Force -ErrorAction SilentlyContinue | Select-Object FullName
    Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue | Where-Object DisplayName -like 'JobniacFast*'
    ```
    Allowed after the first run (by name): the WebView2 user-data folder under
    `%LOCALAPPDATA%\io.github.jonhhh09.jobniacfast` (the name is set by the bundle identifier; confirm
    it exists after the first run and list it in the record), and any log folder the app writes there.
    Not allowed: anything else under `%LOCALAPPDATA%` or `%APPDATA%` matching the names above, and a
    registry uninstall entry (expected: none).
    Owner decision: `%LOCALAPPDATA%\Jobniac\generated` exists if you ran step 20. Either delete it
    by hand and record that, or add it to the allow-list for this run. The CI smoke never creates it.

## 14. Record and close

27. Write the run record: installer name and digest, VM type and Windows build
    (`[System.Environment]::OSVersion`), every step with PASS or FAIL, the protocol version, the
    observed tool count, and the residue list. Restore the snapshot.

A release is not ready for publication until every step is PASS, or each FAIL is fixed and the run is repeated.

## macOS (Apple Silicon)

The Windows steps above do not apply. Run this on an Apple Silicon Mac (macOS 14 or later) that has never
run the app, with the DMG and `SHA256SUMS` from the draft, kept outside the repository. CI runs the scripted
version (`scripts/release/macos_smoke.py`, job `clean-machine-macos`); this is the manual run for the owner.

M1. Check the digest. Expected: equal to the `SHA256SUMS` line.
    `shasum -a 256 JobniacFast_<version>_aarch64.dmg` and `grep aarch64.dmg SHA256SUMS`.
M2. Open the DMG, accept the licence and drag JobniacFast to Applications. Open it from Applications.
    Expected, Developer ID release: it opens with no warning. Expected, ad-hoc release: macOS says it
    cannot verify the app; choose **Open Anyway** in System Settings, Privacy & Security, then open it again.
M3. Check the signature. Expected: no output from `codesign --verify --deep --strict /Applications/JobniacFast.app`;
    for a Developer ID release also `spctl --assess --type execute -v /Applications/JobniacFast.app`
    (accepted, source Notarized Developer ID) and `xcrun stapler validate /Applications/JobniacFast.app`.
M4. Check the layout: `ls /Applications/JobniacFast.app/Contents/Frameworks` lists `libpdfium.dylib`, and
    `ls /Applications/JobniacFast.app/Contents/MacOS` lists `jobniac` and `jobniac-cli`.
M5. Use the app: create a profile, import a PDF on the Documents screen (this loads the bundled PDFium in
    the sandboxed worker), and quit.
M6. CLI: `/Applications/JobniacFast.app/Contents/MacOS/jobniac-cli --version` prints `Jobniac <version>`.
M7. Residue after you drag the app to the Bin: the per-user data under
    `~/Library/Application Support/Jobniac` stays by design, and the WebKit data under
    `~/Library/WebKit/io.github.jonhhh09.jobniacfast` and `~/Library/Caches/io.github.jonhhh09.jobniacfast`
    may remain; record them. Restore the snapshot.
