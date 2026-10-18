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
     in `ci.yml`: `rust` (both systems), `supply-chain`, `ui`, `bindings` and
     `postgres-18`. The names in `ci.yml` today are `ui`, `rust`, `supply-chain` and `postgres`; the
     `bindings` check is a step of `rust`, and the `postgres-18` job is required on the promotion
     pull request. Confirm the names in the GitHub picker before you save.
  3. **Pull request rules:** pull requests into `development` and `main`, one approval if you
     want one (single maintainer; owner choice).
  4. **Tag protection:** tags `v*` restricted to the owner, because the release tag must point at the
     promotion merge commit.
  5. **Code scanning and Scorecard:** available for this repository only once it is public, or with
     GitHub Advanced Security on the account (not verified; see G8).
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

## G4 First draft

- **Who:** owner. **Where:** GitHub Actions and Releases.
- **What:** the release workflow (`release.yml`, as written when this checklist was written) runs when a
  promotion pull request from `development` into `main` is merged in the same repository. It creates a
  **draft** only. Its jobs in order: `checks` (the CI gates on the merge commit), `prepare` (read-only:
  version and tag rules), `draft` (the only job that creates the draft), `build` (Windows and Linux),
  `clean-machine` (the scripted smoke on a fresh runner), `attach` (writes `SHA256SUMS`), `attest`
  (provenance, public repositories only) and `verify-draft` (a final API check).
  The version rules: `X.Y.Z` gives tag `vX.Y.Z` and a stable release; `X.Y.Z-dev.N` gives tag
  `vX.Y.Z-dev.N` and a prerelease.
- **Expected:** `verify-draft` prints its single PASS line. The draft contains the installer, the
  Linux bundles, `SHA256SUMS`, the SBOM and the notices.
- **Failure:** any job red. A failed `clean-machine` leaves the draft in place; the next promotion
  prunes the stale Windows assets and rebuilds it. Read the failed step's log before you re-run.
- **Open:** whether `tauri-action` attaches to an existing draft by ID or commitish is to be confirmed
  when the release workflow is first run.

## G5 Clean-machine run on another machine

- **Who:** owner. **Where:** a Windows 11 virtual machine or Windows Sandbox that you control.
- **What:** the [clean-machine smoke checklist](clean-machine-smoke-checklist.md), with the installer from the
  draft (digest checked against the draft's `SHA256SUMS`).
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
  unpublished. To cut a release, bump on `development` first with
  `py -I scripts/release/release.py bump X.Y.Z` (then `cargo update -w` if it was not run) and open the
  `chore: release X.Y.Z` pull request. Then announce where you choose. The announcement text is yours.
- **Expected:** the release is public, the installer digest matches `SHA256SUMS`, and the release notes
  include the SmartScreen guidance from the [README](../../README.md#install).
- **Failure:** a job fails, so nothing is published. Read the log, fix, and re-run; a rerun reuses the
  draft. A digest mismatch: start again from G4.

## Open questions for the owner

These are not facts in the repository. Each needs an answer from you before the gate it belongs to.

1. Which MCP protocol version does the release snapshot pin: `2025-06-18` or another
   value? The server negotiates up to `2025-11-25`. ([clean-machine smoke, step 22](clean-machine-smoke-checklist.md#11-mcp-exercise).)
2. Whether `JOBNIAC_DB_PASSWORD` goes into each client's JSON file or into a user environment variable
   ([MCP client registration, section 1](mcp-client-registration.md#1-database-settings)).
3. Whether `%LOCALAPPDATA%\Jobniac\generated` is an allowed residue of the manual smoke, or is removed by hand
   ([clean-machine smoke, step 26](clean-machine-smoke-checklist.md#13-uninstall-and-residue)).
4. Which signing route (G3) and who applies for it.
5. ~~Whether the repository keeps the `CLAUDE.md` and `.claude/skills/` tree published or drops them.~~
   Decided: dropped. Both are untracked and listed in `.gitignore`; older commits still contain them.
6. Whether the `CODEOWNERS` file is needed, and with which handle.

## Further gates

Each one is decided in writing before the gate it belongs to.

- **G10 PostgreSQL on pull requests.** Decide whether the `postgres` job runs on every pull request or stays on pushes and the release. Today it does not run on pull requests.
- **G11 Privacy history.** Decide between a history rewrite and acceptance before G7 step 3. Until then the `privacy-scan` job of `ci.yml` fails on pushes and pull requests. A rewrite changes every commit id, so every open branch and pull request must be redone.
- **G12 Installed-application checks before G5.** On an installed desktop build, exercise what the hosted smoke cannot: the 60-second timeout and the refusal paths of the smoke hook, a PDF extraction from the installed application, and an NSIS install with the silent switch confirmed on the built installer. Record each result outside the repository.
- **G13 Linux results.** Read the logs of the first `ci.yml` run (the `Rust (ubuntu-26.04)` job and the `postgres` job), the Linux leg of `e2e.yml` and the Linux leg of `release.yml`. The Linux-only code is verified by CI only.
- **G14 Notice items.** Decide whether the PDFBox Apache-2.0 notice, the HTML4 NOTICE and the W3C entity-set licence (G6, point 3) are needed in `THIRD-PARTY-NOTICES.txt`, and record the decision with G6.
- **G15 Two behaviour choices.** Decide the smoke outcome for a DOWN health report (exit 0 now) and the artifact and generation controls when their services are absent (not rendered now).
