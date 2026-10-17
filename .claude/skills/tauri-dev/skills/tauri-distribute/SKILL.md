---
name: tauri-distribute
description: "Covers building, bundling, signing and publishing Tauri 2 apps: Windows installers (NSIS -setup.exe, MSI/WiX), WebView2 install modes, Windows code signing (signCommand, certificateThumbprint, Azure Key Vault, Artifact Signing), Microsoft Store, macOS .app/DMG/notarization, App Store, Linux AppImage/deb/rpm/AUR/Flatpak/Snap, Google Play, CrabNebula Cloud, and GitHub Actions release workflows with tauri-action. Load for: build installer, sign app, publish to a store, release workflow, `tauri build --bundles`, `--no-bundle`, `tauri bundle`, `bundle.windows`, `bundle.targets`, cross-compiling to Windows, cargo-xwin, SmartScreen."
---

# Distributing Tauri apps

These instructions and the files in `references/` are local copies of the official Tauri v2 docs (https://v2.tauri.app/distribute/, tauri-docs 4b74da6, 2026-10-13). Work only from them: before changing bundle config or CI, read the reference file(s) for the target and cite the page URL from the file's `> Source:` header. If the references don't cover something, say so and point to the live page instead of filling the gap from memory.

## Project context

- JobniacFast is a Tauri 2 app; `src-tauri/tauri.conf.json` has `"bundle": {"active": true, "targets": "all", "icon": [...]}`, identifier `io.github.jonhhh09.jobniacfast`, version 0.1.0.
- Release builds run `pnpm tauri build`; the primary target is Windows (build on a Windows machine or GitHub Actions `windows-latest`).
- The version comes from `tauri.conf.json > version` (see Key rules), so bump it there before a release.
- Windows-specific bundle settings go under `bundle.windows` (`nsis`, `wix`, `webviewInstallMode`, `signCommand`, ...). Full key lists live in the live config reference, which is not in `references/`.

## Key rules

### General
- `tauri build` bundles for the configured formats automatically; to split build and bundle use `tauri build --no-bundle`, then `tauri bundle --bundles <list>` (overview.md).
- `tauri build`, `tauri android build` and `tauri ios build` are the build commands; with pnpm run them as `pnpm tauri build` (overview.md).
- App version: `tauri.conf.json > version` is the recommended place; if unset Tauri falls back to `package > version` in `src-tauri/Cargo.toml` (overview.md).
- Some platforms restrict the version string; check the page for each target (overview.md).
- Most platforms require code signing (overview.md).
- A separate config file can be merged at bundle time with `--config <file>` to keep store-specific settings out of the main config (microsoft-store.md, app-store.md).
- `tauri icon <png-or-svg>` generates all required icons, including Microsoft Store icons (microsoft-store.md).
- GUI apps on macOS and Linux do not inherit `$PATH` from shell dotfiles; Tauri's `fix-path-env-rs` crate addresses this (appimage.md, debian.md, rpm.md, dmg.md).

### Windows
- Windows output is either `.msi` (WiX Toolset v3) or `-setup.exe` (NSIS) (windows-installer.md).
- `.msi` can only be built on Windows; NSIS can be cross-compiled (windows-installer.md).
- MSI builds (`"targets": "msi"` or `"all"`) need the VBSCRIPT optional Windows feature; the error `failed to run light.exe` is the hint to enable it (windows-installer.md).
- Cross-compiling to Windows (Linux/macOS) is a last resort, less tested. It needs NSIS, LLVM/LLD (`lld llvm`, or `brew install llvm`), `rustup target add x86_64-pc-windows-msvc` and `cargo install --locked cargo-xwin`; build with `--runner cargo-xwin --target x86_64-pc-windows-msvc`, output in `target/x86_64-pc-windows-msvc/release/bundle/nsis/`. `XWIN_CACHE_DIR` shares the SDK download (windows-installer.md).
- 32-bit: `rustup target add i686-pc-windows-msvc`, then `tauri build --target i686-pc-windows-msvc`. ARM64: install the "C++ ARM64 build tools" in Visual Studio Installer, `rustup target add aarch64-pc-windows-msvc`, build with `--target aarch64-pc-windows-msvc`; the NSIS installer itself stays x86 under emulation (windows-installer.md).
- `TAURI_BUNDLER_WIX_FIPS_COMPLIANT=true` makes the MSI bundle FIPS compliant (windows-installer.md).
- WebView2 default is `downloadBootstrapper` (0 MB, needs internet). Other `bundle.windows.webviewInstallMode.type` values: `embedBootstrapper` (~1.8 MB), `offlineInstaller` (~127 MB, no internet needed), `fixedRuntime` (~180 MB, needs `path` to the extracted runtime), `skip` (app will NOT work without the runtime) (windows-installer.md).
- `bundle.windows.minimumWebview2Version` makes the installer verify the WebView2 version and run the bootstrapper if it does not match (windows-installer.md).
- Windows 7: MSI is not supported by default because of the bootstrapper download; use `embedBootstrapper` or the NSIS `-setup.exe`. The notification plugin needs the `windows7-compat` Cargo feature (windows-installer.md).
- NSIS: `bundle.windows.nsis.installMode` defaults to per-user (installs to `%LOCALAPPDATA%`, no admin); other values: `perMachine` (system-wide, needs admin) or `both` (user chooses, installer needs admin) (windows-installer.md).
- NSIS hooks: `bundle.windows.nsis.installerHooks` points to an `.nsh` file defining `NSIS_HOOK_PREINSTALL`, `NSIS_HOOK_POSTINSTALL`, `NSIS_HOOK_PREUNINSTALL`, `NSIS_HOOK_POSTUNINSTALL`. Inside a hook macro `${__FILEDIR__}` is the generated script's dir, so `!define` the hook dir at top level first (windows-installer.md).
- NSIS languages: `bundle.windows.nsis.languages`, custom translation files, `displayLanguageSelector: true`; a custom `.nsi` template is `bundle.windows.nsis.template` (windows-installer.md).
- WiX: `bundle.windows.wix.language` (string, array or object with `localePath`), `fragmentPaths` plus `componentRefs`/`componentGroupRefs`/`featureRefs`/`featureGroupRefs`/`mergeRefs`, custom template via `bundle.windows.wix.template` (windows-installer.md).
- Installer-time dependencies (for example VC++ Redistributable): bundle the installer in `resources`, check if already installed, run silently with `/norestart`, clean up (windows-installer.md).
- Signing is not required to run on Windows, but unsigned downloads trigger a SmartScreen warning and signing is required for the Microsoft Store (sign-windows.md).
- Since 2024 EV and OV certificates build SmartScreen reputation the same way; sign every release with the same certificate so reputation carries over (sign-windows.md).
- It must be a code signing certificate; SSL certificates do not work (sign-windows.md).
- Local signing with a certificate in the store: set `bundle.windows.certificateThumbprint`, `digestAlgorithm` (likely `sha256`) and `timestampUrl`; the build log shows `signing app` / `Successfully signed` (sign-windows.md).
- Any other signer (Azure Key Vault via `relic`, Azure Artifact Signing via `artifact-signing-cli`, EV tokens) uses `bundle.windows.signCommand` with `%1` as the file placeholder (sign-windows.md).
- When cross-compiling Windows installers on Linux/macOS you MUST use a custom `signCommand`; the default signer only works on Windows (sign-windows.md, windows-installer.md).
- Azure env vars for both Azure methods: `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_CLIENT_SECRET` (sign-windows.md).

### Microsoft Store
- Tauri only produces EXE/MSI, so the Store product is "EXE or MSI app" that links to a hosted installer; that installer must be offline (`offlineInstaller`), handle auto-updates and be code signed (microsoft-store.md).
- Win32 installers must install silently: NSIS `-setup.exe` uses `/S` (uppercase), MSI uses `/quiet`; enter it as the silent install argument in Partner Center (microsoft-store.md).
- Publisher name cannot equal the product name; if `bundle.publisher` is unset Tauri derives it from the second part of the identifier, so set `bundle.publisher` explicitly when they clash (microsoft-store.md).

### macOS
- macOS builds require a Mac; `tauri build --bundles app` makes the `.app`, `--bundles dmg` the DMG (macos-application-bundle.md, dmg.md).
- Developer ID Application certificate for outside-the-store distribution (needs notarization), Apple Distribution for the App Store (sign-macos.md).
- Signing identity: `bundle.macOS.signingIdentity` or `APPLE_SIGNING_IDENTITY`; CI certificate via `APPLE_CERTIFICATE` (base64 `.p12`) and `APPLE_CERTIFICATE_PASSWORD` (sign-macos.md).
- Notarization env vars: either `APPLE_API_ISSUER` + `APPLE_API_KEY` + `APPLE_API_KEY_PATH`, or `APPLE_ID` + `APPLE_PASSWORD` (app-specific password) + `APPLE_TEAM_ID`; `--skip-stapling` skips stapling (sign-macos.md).
- Ad-hoc signing: `"signingIdentity": "-"`; it does not stop Gatekeeper prompts (sign-macos.md).
- Entitlements: `bundle.macOS.entitlements`; extra bundle files `bundle.macOS.files`; frameworks `bundle.macOS.frameworks`; `minimumSystemVersion` defaults to macOS 10.13 (macos-application-bundle.md).
- DMG: `bundle.macOS.dmg` (`background`, `windowSize` default 660x400, `windowPosition`, `appPosition`, `applicationFolderPosition`); icon sizes/positions are not applied on CI (dmg.md).

### Linux
- Build on the oldest base system you support that has WebKitGTK 4.1 (Ubuntu 22.04 or Debian 12) to avoid `GLIBC_x.y not found` errors (appimage.md, debian.md, rpm.md).
- AppImage: 70+ MB instead of 2-6 MB; `bundle.linux.appimage.bundleMediaFramework` for GStreamer media (best on Ubuntu); `bundle.linux.appimage.files` destinations must begin with `/usr/`; ARM AppImages cannot be cross-compiled (appimage.md).
- AppImage signing env vars: `SIGN=1`, `SIGN_KEY`, `APPIMAGETOOL_SIGN_PASSPHRASE` (required in CI), `APPIMAGETOOL_FORCE_SIGN=1`; AppImage itself does not verify the signature (sign-linux.md).
- Debian: `bundle.linux.deb` (extra dependencies, `files`); default deps `libwebkit2gtk-4.1-0`, `libgtk-3-0`, plus `libappindicator3-1` for tray (debian.md).
- RPM: `bundle.linux.rpm` (`preInstallScript`, `postInstallScript`, `preRemoveScript`, `postRemoveScript`, `conflicts`, `depends`, `obsoletes`, `provides`, `desktopTemplate`, `epoch`, `files`); signing via `TAURI_SIGNING_RPM_KEY` and `TAURI_SIGNING_RPM_KEY_PASSPHRASE` (rpm.md).

### Mobile and stores
- iOS/macOS App Store: bundle ID in App Store Connect must equal `identifier`; macOS needs `bundle.category`, a provisioning profile at `embedded.provisionprofile`, `ITSAppUsesNonExemptEncryption` in Info.plist and the App Sandbox entitlement; upload a `.pkg` signed with a Mac Installer Distribution certificate via `xcrun altool` (app-store.md).
- iOS: `tauri ios build --export-method app-store-connect`; manual signing env vars `IOS_CERTIFICATE`, `IOS_CERTIFICATE_PASSWORD`, `IOS_MOBILE_PROVISION` (app-store.md, sign-ios.md).
- Android: `tauri android build --aab` for Google Play (`--apk`, `--target`, `--split-per-abi` optional); the first upload must be manual; signing uses `src-tauri/gen/android/keystore.properties` plus Gradle config (google-play.md, sign-android.md).
- Flatpak, Snap and AUR have step-by-step guides only (flatpak.md, snapcraft.md, aur.md).

### CI
- `tauri-apps/tauri-action@v1` runs `tauri build` and creates a GitHub release; keep it as the last workflow step (pipelines-github.md).
- Job needs `permissions: contents: write`; a "Resource not accessible by integration" error means the token needs read and write workflow permissions (pipelines-github.md).
- Triggers: push to a `release` branch (default example) or a version tag such as `app-v*` (pipelines-github.md).
- `projectPath` input when the app is not at the repo root; `args` input carries `--target ...` (pipelines-github.md).
- Linux runner deps in the example: `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf xdg-utils`; `ubuntu-22.04-arm` runners are only available in public repos (pipelines-github.md).
- CrabNebula Cloud integrates with the Tauri updater and has a CLI and a GitHub Action; details are in its own docs (crabnebula-cloud.md, pipelines-crabnebula-cloud.md).

## How to build a Windows installer
1. Read [windows-installer.md](references/windows-installer.md).
2. On Windows run `pnpm tauri build` (all configured targets) and find the `.msi` and `-setup.exe` in the bundle output.
3. Pick `bundle.windows.webviewInstallMode` (default `downloadBootstrapper`) and, for NSIS, `bundle.windows.nsis.installMode`.
4. For MSI, make sure VBSCRIPT is enabled; for another CPU use `--target` (i686/aarch64) after `rustup target add`.
5. Optional customization: NSIS `installerHooks`, languages, `displayLanguageSelector`; WiX `fragmentPaths`/`language`.

## How to sign Windows builds
1. Read [sign-windows.md](references/sign-windows.md). Note its OV guide only applies to certificates acquired before June 1st 2023; for others follow the issuer docs plus `signCommand`.
2. Local certificate: add `certificateThumbprint`, `digestAlgorithm`, `timestampUrl` under `bundle.windows`; run `pnpm tauri build` and look for `Successfully signed`.
3. Other tools: set `bundle.windows.signCommand`, for example `relic sign --file %1 --key azure --config relic.conf` (Azure Key Vault) or `artifact-signing-cli -e <endpoint> -a <account> -c <profile> -d <desc> %1`; export the `AZURE_*` env vars.
4. CI: store `WINDOWS_CERTIFICATE` (base64 `.pfx` from `certutil -encode`) and `WINDOWS_CERTIFICATE_PASSWORD` as secrets; decode with `certutil -decode` and `Import-PfxCertificate` into `Cert:\CurrentUser\My` before the build step (`if: matrix.platform == 'windows-latest'`).

## How to set up a GitHub Actions release
1. Read [pipelines-github.md](references/pipelines-github.md) and copy its example workflow (checkout, Linux deps, setup-node, Rust toolchain + `swatinem/rust-cache`, frontend install, `tauri-apps/tauri-action@v1`).
2. Set the `cache:` option of setup-node to `pnpm` and replace the install step with the pnpm equivalent (the example comments say to change npm/yarn/pnpm to the one you use).
3. Add `permissions: contents: write` and `GITHUB_TOKEN` env; set `tagName: app-v__VERSION__`, `releaseName`, `releaseDraft`.
4. For a Windows-only release, keep only the `windows-latest` matrix entry; add the certificate import step from the signing section above if signing.
5. In repo settings enable read and write workflow permissions if you hit "Resource not accessible by integration".

## How to publish to the Microsoft Store
1. Read [microsoft-store.md](references/microsoft-store.md); enroll and create an "EXE or MSI app" product.
2. Create `src-tauri/tauri.microsoftstore.conf.json` with `webviewInstallMode.type = offlineInstaller`.
3. Run `pnpm tauri build --no-bundle`, then `pnpm tauri bundle --config src-tauri/tauri.microsoftstore.conf.json`.
4. Sign the installer, host it, link it in the product, and enter `/S` (NSIS) or `/quiet` (MSI) as the silent install argument.

## Reference index
| File | Read it when |
|---|---|
| [overview.md](references/overview.md) | Choosing build/bundle commands, versioning, finding the right platform page |
| [windows-installer.md](references/windows-installer.md) | Configuring NSIS/MSI, WebView2 modes, install modes, hooks, cross-compiling, ARM/32-bit |
| [sign-windows.md](references/sign-windows.md) | Signing with a certificate, Azure Key Vault, Artifact Signing, `signCommand` |
| [microsoft-store.md](references/microsoft-store.md) | Publishing a Windows installer through the Microsoft Store |
| [pipelines-github.md](references/pipelines-github.md) | Writing a GitHub Actions release workflow with `tauri-action`, ARM runners |
| [pipelines-crabnebula-cloud.md](references/pipelines-crabnebula-cloud.md) | Using CrabNebula Cloud in a CI pipeline |
| [crabnebula-cloud.md](references/crabnebula-cloud.md) | Evaluating CrabNebula Cloud as CDN/update host |
| [macos-application-bundle.md](references/macos-application-bundle.md) | Info.plist, entitlements, frameworks, `.app` structure |
| [dmg.md](references/dmg.md) | DMG window layout options |
| [sign-macos.md](references/sign-macos.md) | macOS certificates, notarization, ad-hoc signing, CI keychain |
| [app-store.md](references/app-store.md) | Uploading macOS/iOS builds to App Store Connect |
| [sign-ios.md](references/sign-ios.md) | iOS automatic/manual signing, provisioning profiles |
| [google-play.md](references/google-play.md) | Building AAB/APK, version code, minSdkVersion, upload |
| [sign-android.md](references/sign-android.md) | Android keystore and Gradle signing config |
| [appimage.md](references/appimage.md) | AppImage limits, media framework, custom files, ARM |
| [debian.md](references/debian.md) | `.deb` config, custom files, ARM cross-compile |
| [rpm.md](references/rpm.md) | RPM scripts, dependencies, signing, debugging packages |
| [sign-linux.md](references/sign-linux.md) | Signing and validating AppImages with GPG |
| [aur.md](references/aur.md) | Publishing a PKGBUILD to the AUR |
| [flatpak.md](references/flatpak.md) | Flatpak manifests and Flathub submission |
| [snapcraft.md](references/snapcraft.md) | `snapcraft.yaml` and Snap Store release |

## Related skills
- `tauri-develop`: `tauri.conf.json` structure, resources, sidecars, icons.
- `tauri-official-plugins`: the updater plugin (auto-updates, required by the Microsoft Store flow) and other plugins with permission tables.
- `tauri-security`: security hardening of the app.
- `tauri-testing`: WebDriver tests in CI.
