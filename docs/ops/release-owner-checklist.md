# Release owner checklist

The steps that only the owner can do, in the order they must happen. Nothing in the repository runs
these: no workflow publishes a release, pushes a tag or changes a GitHub setting. Each gate says who,
what and where. A gate is done only when you have the evidence written in your own record.

Current state: the release workflows were validated with `actionlint` and by running the scripts they call locally; none has run on a
hosted runner yet.

## G1 Verify that GitHub Actions can run

- **Who:** owner. **Where:** the repository's Actions tab.
- **What:** run the existing `CI` workflow once on a branch you choose and confirm that every job
  starts. Nothing else in this checklist can be checked by a CI run until this is true.
- **Expected:** the Actions tab accepts the run and the jobs execute.
- **Failure:** jobs are refused or stay queued. Read the annotation on the run; the release is blocked
  until Actions can run.

## G2 Repository settings

- **Who:** owner. **Where:** GitHub, repository Settings.
- **What, in this order:**
  1. **Private vulnerability reporting:** enabled (Security tab). The advisory link in
     `.github/ISSUE_TEMPLATE/config.yml` depends on it.
  2. **Branch protection on `development` and `main`:** configured as recorded in
     [branch-protection.md](branch-protection.md) (rulesets, required checks, merge methods). The older
     notes below predate it. Required checks named as the CI jobs are named
     in `ci.yml`: `rust` (both systems), `supply-chain`, `ui` and `bindings`. The names in `ci.yml`
     today are `ui`, `rust` and `supply-chain`; the `bindings` check is a step of `rust`. Confirm the
     names in the GitHub picker before you save. `Rust (macos-latest)` is a required check too (see
     branch-protection.md): add it to both rulesets.
  3. **Pull request rules:** pull requests into `development` and `main`, one approval if you
     want one (single maintainer; owner choice).
  4. **Tag protection:** tags `v*` restricted to the owner, because the release tag must point at the
     promotion merge commit.
  5. **Code scanning and Scorecard:** available for this repository only once it is public, or with
     GitHub Advanced Security on the account (not verified; see G8).
  6. **Wiki:** open the repository's Wiki tab and save a first page once (any placeholder text). GitHub
     creates the wiki repository only then, and neither Actions nor the API can create it. Until it
     exists the `Wiki` workflow fails with "The wiki repository does not exist yet" (its first run, after
     the first promotion, did), and it runs again only when `docs/wiki/`, `scripts/ci/wiki_sync.py` or
     `wiki.yml` changes on `main`. After saving the page, start the `Wiki` workflow from the Actions tab
     (`workflow_dispatch`) so the pages in `docs/wiki/` are mirrored, replacing the placeholder.
- **Failure:** a setting is missing. Do not publish a draft until 1 to 4 are in place.

## G3 Signing route decision

- **Who:** owner. **Where:** decision recorded in your record; applications on the provider's site.
- **What:** choose one signing route, in order of preference:
  1. SignPath Foundation (free for open-source projects; eligibility and GitHub integration not verified).
  2. Azure Artifact Signing through `bundle.windows.signCommand` (cost and eligibility not verified).
  3. An OV code-signing certificate (`certificateThumbprint`, `digestAlgorithm: sha256`, `timestampUrl`).
  4. An Azure Key Vault certificate through relic.
  EV certificates give no instant SmartScreen reputation since 2024; do not buy one for that reason.
  Until a route is in place, the release is unsigned, and the download guidance in the README applies.
- **Secrets:** signing secrets live only in a protected release environment, only for the Windows build
  step, never in pull-request workflows. `release.yml` has NO signing step yet, so no secret name exists:
  adding the chosen route (its own pull request) defines the names. Do not create secrets from this
  checklist.
- **Updater signing key (separate from the Windows code-signing route above).** The in-app updater
  refuses any update that is not signed with this key, and the release stops until it is in place.
  **Done:** the public key is committed as `plugins.updater.pubkey` in
  `src-tauri/tauri.release.conf.json`, and the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` are set; only the tauri-action step of the build reads them.
  `release.py` still refuses the placeholder `REPLACE_WITH_UPDATER_PUBLIC_KEY`, which matters for forks
  (they generate their own key with `pnpm tauri signer generate -w <path outside the repository>`).
  Keep an offline backup of the private key: losing it means installed copies can never update again.
- **Each route is its own pull request with its own gate.** The route is not chosen by this checklist.
- **Failure:** no route decided. Keep the release unsigned and say so in the release notes.

### G3a macOS signing and notarization (Apple Developer ID)

- **Who:** owner. **Where:** the Apple Developer account, then the repository's Settings, Secrets and variables,
  Actions (repository secrets, or the secrets of a protected release environment).
- **What:** `release.yml` signs and notarizes the macOS app by itself once the secrets below exist, and
  builds an ad-hoc signed app until then (macOS then asks the user to allow it by hand; see the README).
  Nothing else changes in the workflow when the secrets arrive. The names are the ones the Tauri
  documentation defines (`distribute/sign/macos`), plus `APPLE_API_PRIVATE_KEY`:
  - **Signing, all three required:**
    - `APPLE_CERTIFICATE`: the base64 of the exported `.p12` of a "Developer ID Application" certificate
      (`openssl base64 -A -in certificate.p12`).
    - `APPLE_CERTIFICATE_PASSWORD`: the password chosen when the `.p12` was exported.
    - `APPLE_SIGNING_IDENTITY`: the certificate name, for example
      `Developer ID Application: <Name> (<TEAMID>)` (`security find-identity -v -p codesigning`).
  - **Notarization, one complete set:**
    - App Store Connect API key (preferred): `APPLE_API_ISSUER` (the issuer ID), `APPLE_API_KEY` (the key
      ID) and `APPLE_API_PRIVATE_KEY` (the full text of the downloaded `AuthKey_<id>.p8`; the build step
      writes it to a private temporary file and exports `APPLE_API_KEY_PATH` for the step).
    - or an Apple ID: `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`.
  - Partly defined secrets fail the macOS leg on purpose (a Developer ID app that is not notarized is
    refused by Gatekeeper). All of them undefined means ad-hoc.
- **Repository variable `MACOS_SIGNING_REQUIRED` (Settings, Secrets and variables, Actions, Variables):**
  set it to exactly `1` once the Apple secrets are in place, so a missing or misspelled secret fails the
  release instead of shipping ad-hoc. With it set, an ad-hoc macOS build fails in `macos_build.sh` before
  building, and `clean-machine-macos` asserts a Developer ID signature. Unset (or any other value), an
  ad-hoc build only emits a `::warning::` annotation on the run. A stable release is published as the latest
  either way, so check the annotation until the variable is set.
- **`JOBNIAC_PDFIUM_DIR` on a Developer ID build:** library validation is on there, so the variable can only
  point at a PDFium build signed by the same team. Ad-hoc and development builds load any library.
- **Where they are read:** only the step "Build the macOS bundles" of the `build` job, through
  `scripts/release/macos_build.sh`. That step does not hold the GitHub token and does not run for a pull
  request from a fork (the release itself runs only for a merged promotion pull request of this
  repository). No secret is echoed, and the notarization key file is deleted when the step ends. The script
  compiles first with `pnpm tauri build --no-bundle` with none of the Apple or updater secrets in the
  environment, then runs `pnpm tauri bundle` with them, so build scripts and proc-macros never see them.
- **Hardened runtime:** the bundler signs with the hardened runtime. A Developer ID build needs no
  entitlement: the bundled PDFium library (`Contents/Frameworks/libpdfium.dylib`) is signed with the same
  identity as the app, so library validation passes. The ad-hoc build adds
  `com.apple.security.cs.disable-library-validation` (`src-tauri/macos/Entitlements.adhoc.plist`), because an
  ad-hoc signature has no Team ID to match.
- **Expected:** the `clean-machine-macos` job prints `INFO: Developer ID signature` and passes its
  Gatekeeper (`spctl --assess`) and stapler checks; it prints `INFO: ad-hoc signature` before the secrets exist.
- **Failure:** `clean-machine-macos` fails at "PDF text through the bundled worker" on a Developer ID build:
  the library and the app do not share a Team ID. Read the step's output before changing any entitlement.

## G4 First draft

- **Who:** owner. **Where:** GitHub Actions and Releases.
- **What:** the release workflow (`release.yml`, as written when this checklist was written) runs when a
  promotion pull request from `development` into `main` is merged in the same repository. It creates a
  **draft** only. Its jobs in order: `checks` (the CI gates on the merge commit), `prepare` (read-only:
  version and tag rules), `draft` (the only job that creates the draft), `build` (Windows, Linux and
  macOS Apple Silicon), `clean-machine` and `clean-machine-macos` (the scripted smokes on fresh runners),
  `attach` (writes `SHA256SUMS`), `attest`
  (provenance, public repositories only) and `verify-draft` (a final API check).
  The version rules: `X.Y.Z` gives tag `vX.Y.Z` and a stable release; `X.Y.Z-dev.N` gives tag
  `vX.Y.Z-dev.N` and a prerelease. The version is computed in `prepare` from the existing `v*` tags and
  never committed: each merged promotion gets the patch after the highest stable tag (the first release
  takes the version in the files). If the version in the files is higher than that and not tagged yet
  (a `bump` to a minor, major or `-dev.N` version on `development`), the release takes the file version
  instead. A tag already on the merge commit keeps its version, so a rerun reuses it. The build legs stamp
  the computed version into their checkout before building. Release notes are the `CHANGELOG.md` section of
  that version when it exists, otherwise GitHub's generated notes since the previous stable tag.
  Runs of merged promotions share one concurrency group, so they serialize and never compute the same
  version. GitHub keeps one pending run per group, so when several promotions merge quickly the middle
  pending run is cancelled; its changes ship with the next release. The run refuses (fails closed) when
  the highest stable tag is not an ancestor of the merge commit, so a stale re-run of an old merge cannot
  publish older code as a newer release. After a `-dev.N` prerelease, the next automatic release is the
  next stable patch of the last stable line.
- **Expected:** `verify-draft` prints its single PASS line. The draft contains the installer, the
  Linux bundles, the macOS DMG and app archive (with their update signatures), `SHA256SUMS`, the SBOM and
  the notices.
- **Dry run on the open promotion pull request:** the same workflow also runs while the promotion pull
  request is open (when it is opened or reopened, and on every push to `development`). A pull request run
  uses the workflow and the scripts of the pull request's merge ref, so a release fix that is on
  `development` is exercised here before the promotion merges. The dry run builds that merge ref, signs
  the updater artifacts with the real key (so it proves that the `TAURI_SIGNING_PRIVATE_KEY` secret
  matches the public key in `tauri.release.conf.json`), builds the macOS leg ad-hoc (no Apple secret),
  runs both clean-machine smokes, and assembles the asset set and `latest.json` into the workflow
  artifact `dry-run-assets` (kept 3 days). It never creates or changes a release, a draft, a tag or an
  attestation, and it skips `checks` and `e2e`, which run on the pull request as `ci.yml` and `e2e.yml`.
  It costs about 25 to 35 minutes of runner time per push to the open promotion pull request (a newer
  push cancels the older dry run). It is not a required check; add `Release` jobs to the `Protect main`
  ruleset only if you want a red dry run to block the merge. Read it before you merge: a red dry run is
  what the merge would have failed on.
- **Expected (dry run):** `prepare`, the three `build` legs, `clean-machine`, `clean-machine-macos` and
  `dry-run-assemble` are green; every other job shows as skipped. A real merge then runs the same
  builds with the draft, the signing and notarization secrets, `attach`, `attest`, `verify-draft` and
  `publish`.
- **Failure:** any job red. A failed `clean-machine` leaves the draft in place; the next promotion
  prunes the stale Windows assets and rebuilds it. Read the failed step's log first. "Re-run" of a failed
  `Release` run is only for transient failures (a runner or network error): a re-run replays the old merge
  commit with the workflow file and scripts of that commit, so it cannot pick up a fix. A code or workflow
  fix goes through `development` and a new promotion pull request, whose new merge commit makes the
  next run prune the stale draft's assets and rebuild them. The build legs do not cancel each other, so
  one run shows every failing leg, and the smoke jobs still run for the legs that built.
- **Closed:** `tauri-action` attaches to the existing draft by release ID (draft 420976097 of the failed
  run 39818216211 shows the installer, the AppImage, the deb and their signatures attached that way).

## G5 Clean-machine run on another machine

- **Who:** owner. **Where:** a Windows 11 virtual machine or Windows Sandbox that you control, and an
  Apple Silicon Mac that has never had the app.
- **What:** the [clean-machine smoke checklist](clean-machine-smoke-checklist.md), with the installer from the
  draft (digest checked against the draft's `SHA256SUMS`); on the Mac, the macOS section of that checklist.
- **Expected:** every step PASS, and the run record written outside the repository.
- **Failure:** any FAIL. Fix it, publish a new draft (a new commit and a new promotion), and repeat.

## G6 Licensing review closure

- **Who:** owner, with a qualified reviewer of your choice. **Where:** your record. This checklist is not legal advice.
- **What:** close the open licensing points.
  Status at 2026-10-16:

  | Point | Subject | Status |
  |---|---|---|
  | 1 | Golden vectors that may count as derived from OpenJDK data | Open. No review is recorded; confirmation is required before a vector set is committed. |
  | 2 | Adobe AFM notice wording for the Standard-14 metrics | Open for sign-off. Provenance is recorded (PDFBox 3.0.7 AFM files, SHA-256 of the jar, notice kept in `crates/pdf/src/metrics/NOTICE-Adobe-AFM.txt`). |
  | 3 | W3C entity-set licence (HTML4 table) | Not recorded. Check before the HTML4 table is regenerated. |
  | 4 | Legal review of the OpenJDK-derived position | Open. Blocking before the first public release. |
  | 5 | Bundle identifier and handle | Decided. |

- **Expected:** each open point is closed with a dated note that says who decided and on what evidence.
- **Failure:** any open point is still open. The release stays a draft and is not made public.

## G7 Public-push audit and privacy scan

- **Who:** owner. **Where:** your working copy, before the first push to a public remote.
- **What, in order:**
  1. Audit the tree before the first public push: fixtures and evidence JSON,
     owner URLs, local paths, private-note references, attribution copies, `CODEOWNERS`.
  2. Create the gitignored `.privacy-local.json` with the owner literals and run the privacy scan
     over the working tree and the full history:
     ```powershell
     py -I scripts/ci/privacy_scan.py --require-local
     ```
     Expected: exit code 0 and no findings. Exit 1 lists masked findings; exit 2 is a configuration error.
     Keep any `--report` output outside the repository.
  3. Decide whether the history needs rewriting before publication. Any rewrite is a separate decision
     and changes every commit id.
- **Failure:** any finding not allow-listed after review. Fix the content and re-run.

## G8 Make the repository public

- **Who:** owner. **Where:** GitHub, Settings, Danger zone, Change visibility.
- **What:** only after G1 to G7. Consequences: artifact attestations start for the release
  (public repositories only); Scorecard results and the badge, and code scanning, depend on the visibility. Private
  vulnerability reporting must be enabled after the change if it is not already on.
- **Expected:** the repository is public and the first Scorecard and CodeQL runs complete (check the logs).
- **Failure:** a run fails on visibility. Read the log before you treat it as a regression.

## G9 CI publishes after the gates, then announce

- **Who:** `release.yml` publishes; the owner announces. **Where:** GitHub Releases.
- **What:** the last job (`publish`) runs only when every other job has passed, including the draft
  verification (exact asset set, digests, `latest.json` and the update signatures). It creates the tag
  `vX.Y.Z` on the merge commit (it fails if the tag exists on another commit) and publishes the draft:
  the latest release for `X.Y.Z`, a prerelease for `X.Y.Z-dev.N` (never the latest, so the updater
  ignores it). To stop a release, do not merge the promotion pull request; a failed job leaves the draft
  unpublished. To cut a patch release, merge the promotion pull request: the version is automatic. To
  cut a minor, major or `-dev.N` release, bump on `development` first with
  `py -I scripts/release/release.py bump X.Y.Z` (then `cargo update -w` if it was not run) and open the
  `chore: release X.Y.Z` pull request; the bumped version is the floor of the next release. Then announce
  where you choose. The announcement text is yours.
- **Expected:** the release is public, the installer digest matches `SHA256SUMS`, and the release notes
  include the SmartScreen guidance from the [README](../../README.md#install).
- **Failure:** a job fails, so nothing is published. Read the log. For a transient runner or network
  failure, "Re-run failed jobs" reuses the draft; for anything in the code or the workflow, fix it on
  `development` and promote again (see G4: a re-run replays the old merge commit). A digest mismatch: start
  again from G4.

## Open questions for the owner

These are not facts in the repository. Each needs an answer from you before the gate it belongs to.

1. Which MCP protocol version does the release snapshot pin: `2025-06-18` or another
   value? The server negotiates up to `2025-11-25`. ([clean-machine smoke, step 22](clean-machine-smoke-checklist.md#11-mcp-exercise).)
2. Whether the default database file (`%LOCALAPPDATA%\Jobniac\data\jobniac.sqlite3`) is an allowed
   residue of the manual smoke, or is removed by hand
   ([clean-machine smoke, step 26](clean-machine-smoke-checklist.md#13-uninstall-and-residue)).
3. Whether `%LOCALAPPDATA%\Jobniac\generated` is an allowed residue of the manual smoke, or is removed by hand
   ([clean-machine smoke, step 26](clean-machine-smoke-checklist.md#13-uninstall-and-residue)).
4. Which signing route (G3) and who applies for it.
5. ~~Whether the repository keeps the `CLAUDE.md` and `.claude/skills/` tree published or drops them.~~
   Decided: dropped. Both are untracked and listed in `.gitignore`; older commits still contain them.
6. Whether the `CODEOWNERS` file is needed, and with which handle.

## Further gates

Each one is decided in writing before the gate it belongs to.

- **G11 Privacy history.** Decide between a history rewrite and acceptance before G7 step 3. Until then the `privacy-scan` job of `ci.yml` fails on pushes and pull requests. A rewrite changes every commit id, so every open branch and pull request must be redone.
- **G12 Installed-application checks before G5.** On an installed desktop build, exercise what the hosted smoke cannot: the 60-second timeout and the refusal paths of the smoke hook, a PDF extraction from the installed application, and an NSIS install with the silent switch confirmed on the built installer. Record each result outside the repository.
- **G16 macOS results.** Read the logs of the first `Rust (macos-latest)` run and of the macOS leg and the
  `clean-machine-macos` job of `release.yml`. The macOS-only code and the DMG step (`bundle_dmg.sh` drives
  Finder) are verified by CI only on a hosted runner; a first run may need a retry if the DMG step fails
  on the runner.
- **G13 Linux results.** Read the logs of the first `ci.yml` run (the `Rust (ubuntu-26.04)` job), the Linux leg of `e2e.yml` and the Linux leg of `release.yml`. The Linux-only code is verified by CI only.
- **G14 Notice items.** Decide whether the PDFBox Apache-2.0 notice, the HTML4 NOTICE and the W3C entity-set licence (G6, point 3) are needed in `THIRD-PARTY-NOTICES.txt`, and record the decision with G6.
- **G15 Two behaviour choices.** Decide the smoke outcome for a DOWN health report (exit 0 now) and the artifact and generation controls when their services are absent (not rendered now).
