# Branch protection and merge rules

How the repository is locked down. The settings live in GitHub (repository rulesets and settings),
not in files, so this page is the record of what they should be.

## Flow

```text
issue branch ──PR (squash, auto-merge)──▶ development ──PR (merge commit, owner)──▶ main ──▶ release draft
```

- Work happens on an issue branch cut from `development`.
- The issue branch is merged into `development` by squash. Auto-merge is switched on automatically
  (`.github/workflows/automerge.yml`) and fires once the required checks pass. GitHub deletes the
  issue branch after the merge.
- `development` is promoted to `main` by a pull request that uses a merge commit, so the two
  branches never diverge. Auto-merge is not enabled on it: merging into `main` starts
  `release.yml`, so the owner merges it deliberately. When every gate passes, its last job tags the
  merge commit and publishes the release (the latest release for a stable version); a failed gate
  leaves the draft unpublished. The release version is computed from the existing `v*` tags (the next
  patch) and never committed, so the protected branches need no version commit; a manual `bump` on
  `development` only raises the floor.

## Rulesets

| Ruleset | Branch | Rules |
|---|---|---|
| Protect development | `development` | no deletion, no force push, pull request required (squash only, conversations resolved), required status checks, branches must be up to date |
| Protect main | `main` | the same, merge commit only |

There is no bypass actor: direct pushes are blocked for everyone, including the owner.

Required status checks (named as the jobs are named in the workflows):

| Branch | Required checks |
|---|---|
| `development` | `Branch policy`, `CI result (shared)`, `CI result (platforms)`, `E2E result` |
| `main` | `Branch policy`, `CI result (shared)` |

The three `result` jobs are aggregators (`result-shared` and `result-platforms` in `ci.yml`, `result`
in `e2e.yml`). Each runs on every run (`if: always()`), so a cancelled or failed dependency never
leaves it skipped (a skipped required check counts as passed), prints a table of its dependencies to
the step summary and fails unless every dependency is `success` or `skipped`. A failed `Changes` job
fails them too. They have no checkout and use only bash and `jq`.

- `CI result (shared)` needs `Changes`, `UI`, `Supply chain (cargo-deny)`, `Release tool unit tests`,
  `Privacy scan (working tree and history, generic classes)` and `Text (spelling, Markdown, links)`.
- `CI result (platforms)` needs `Changes`, `Rust (ubuntu-26.04)`, `Rust (macos-latest)` and
  `Rust (windows-latest)`.
- `E2E result` needs `Changes`, `E2E (ubuntu-26.04)` and `E2E (windows-latest)`.

The individual job names (`UI`, `Rust (...)`, `E2E (...)` and so on) are no longer required checks. The
owner switches the rulesets by hand after the change that introduced the aggregators merges: add the
new names and remove the old seven (`Branch policy` stays; `UI`, `Rust (ubuntu-26.04)`,
`Rust (macos-latest)`, `Supply chain (cargo-deny)`, `Release tool unit tests` and
`Privacy scan (working tree and history, generic classes)`) in the same edit, so no pull request waits
for a check that no longer exists.

Linux jobs run on the latest Ubuntu LTS runner (`ubuntu-26.04`). The one exception is the Linux leg
of the release build in `release.yml`, which stays on `ubuntu-24.04` because the build host sets the
glibc floor of the AppImage and deb. When the runner moves to a new LTS, the `Rust (ubuntu-...)` and
`E2E (ubuntu-...)` job names change; the required checks are the aggregators, so no ruleset edit is
needed, but check the doc and the job names in `ci.yml` and `e2e.yml`. The macOS job runs
on `macos-latest` (Apple Silicon); when GitHub moves that label to a new macOS version the job name stays,
but a runner that is no longer arm64 fails the release leg, which checks the architecture.

`Rust (windows-latest)` and `E2E (windows-latest)` run on pull requests whose change classes need them
(below), and on every full run. The WebDriver suite has no macOS leg (`tauri-driver` has no macOS
driver). The end-to-end suite is a release
gate (`release.yml` needs it), so a promotion pull request whose `E2E (...)` checks are red must not
be merged: the release would stop after the merge.

### Change classes and the gate

A required check whose job is skipped counts as passed, so `ci.yml` and `e2e.yml` skip work safely.
The `Changes` job (`scripts/ci/gate.sh`) decides which jobs run and writes a short summary (the kind of
run, the number of changed files, the classes, and for each job whether it runs and why).

Which runs are path-filtered: only a plain pull request, that is a `pull_request` event that is neither
the release call nor a promotion pull request. A promotion pull request has base `main`, head
`development` and a head repository equal to this one. Every other pull request into `main` is plain,
for example a Dependabot security update (they always target the default branch, and `Branch policy`
rejects them anyway). Pushes, manual runs (`workflow_dispatch`, the nightly dispatch included),
promotion pull requests and the release call run the full set, minus the jobs whose marker is trusted
(below).

Each changed path takes the classes of the first matching rule of `scripts/ci/change_classes.toml`,
read by `scripts/ci/classify.py` (see `scripts/ci/README.md` to add a rule). A path that no rule
matches is `unknown`, and `unknown` turns every job on. The classes and what they switch on:

| Class | Paths (the first matching rule wins) | Jobs |
|---|---|---|
| `prose` | `**/*.md`, `docs/**`, `LICENSE`, issue templates, `CODEOWNERS`, `dependabot.yml`, `.vscode/**`, `.gitignore`, `osv-scanner.toml`, the text check configuration, the other workflows, `scripts/ci/**` | none (the privacy scan, the text checks and the release tool tests always run) |
| `changelog` | `CHANGELOG.md` | `Release tool unit tests` |
| `release` | `release.yml`, `scripts/release/**`, `tauri.release.conf.json`; also root `Cargo.toml`, `package.json`, `tauri.conf.json`, which `release.py check-versions` reads | `Release tool unit tests` |
| `ui` | `src/**`, `index.html`, `public/**`, Vite, TypeScript and biome configuration, pnpm files | `UI`, both `E2E` jobs |
| `ui-tests` | `playwright/**`, `playwright.config.ts` | `UI` |
| `e2e-tests` | `e2e/**` | `UI` (biome lints `e2e/`), both `E2E` jobs |
| `rust` | `crates/**`, `src-tauri/**`, `Cargo.lock`, `Cargo.toml` files, toolchain and lint configuration, `tools/**`, `scripts/fetch-pdfium*`, and every file under a `tests/fixtures` or `golden` directory | the three `Rust` jobs, both `E2E` jobs |
| `supply-chain` | `deny.toml`, `Cargo.lock`, `Cargo.toml` files | `Supply chain (cargo-deny)` |
| `ci-all`, `e2e-all` | `ci.yml`, `e2e.yml` | every job of that workflow |
| `all` | `.github/actions/**`, `gate.sh`, the classifier and its rule file | every job of both workflows |
| `unknown` | anything no rule matches, and `.gitattributes` (line endings affect every job) | every job of both workflows |

`src/ipc/**` is both `ui` and `rust` (the Linux Rust job regenerates `src/ipc/bindings.ts` and diffs it).
A fixture or golden file is `rust` even when it is Markdown: tests read whole fixture directories. The
UI changes switch no Rust job on, because no Rust test reads the UI; the `E2E` jobs embed it and run.

- The privacy scan, `Text (spelling, Markdown, links)` and `Release tool unit tests` always run
  (subject to the release-call scope). The release tool tests are always on because they read many
  repository files (the platform bundle configs, entitlements, `Cargo.lock`, crate manifests, the
  licence and notice texts, the workflows) that no path rule would keep track of, and they take
  seconds; the `changelog` and `release` classes still name them. The privacy scan runs the unit
  tests of `scripts/ci/tests`, which include the classifier's.
- Force a full run on a branch: `gh workflow run ci.yml --ref <branch>` and
  `gh workflow run e2e.yml --ref <branch>`. A manual run ignores markers and runs everything.
- Every job that is a required check must keep a literal `name:`, and the aggregators must keep
  `if: always()`. Do not turn the Rust or E2E jobs back into a matrix: a skipped matrix job reports its
  unexpanded name.
- The gate fails open: every heavy job runs when the `Changes` job did not succeed
  (`needs.changes.result != 'success'`), because a job skipped behind a failed gate would otherwise
  count as passed. The aggregators fail in that case, so the check stays red.

### Passed markers

- `.github/workflows/nightly.yml` runs daily at 02:30 UTC and on demand, and dispatches `ci.yml` and
  `e2e.yml` on `development` with `workflow_dispatch`. Merges into `development` are made by
  auto-merge with the workflow token, and GitHub starts no workflow for those, so `development` has
  no push run. The dispatched runs save the Rust caches under `development`, which every pull
  request into `development` restores from, and save the passed marker for the `development` tip.
  GitHub fires schedules from the default branch (`main`) only, so the nightly starts once this file
  is on `main`; until then, start `ci.yml` and `e2e.yml` on `development` by hand
  (`gh workflow run ci.yml --ref development`).
- Rust caches are saved only by runs that are not pull requests (`save-if`); pull requests only
  restore them. This keeps the Actions cache under its 10 GB limit, so markers and base caches are
  not evicted.
- Each heavy job has its own marker, an Actions cache entry keyed by the git tree SHA:
  `ci-passed-ui-<tree>`, `ci-passed-rust-linux-<tree>`, `ci-passed-rust-macos-<tree>`,
  `ci-passed-rust-windows-<tree>`, `e2e-passed-linux-<tree>` and `e2e-passed-windows-<tree>`. Supply
  chain, release tools, the privacy scan and the text checks have none: they are cheap (and, except
  supply chain, always on) and rerun.
- `Mark tree as passed` (one job per workflow) saves the marker of each heavy job that ran and
  succeeded in that run. A red Windows job therefore does not block the Linux and macOS markers. It
  saves on a push, a manual or nightly run on `development` or `main`, and on the promotion pull
  request, never in the release call.
- A job whose marker is trusted is skipped. The gate trusts a marker saved on `development` or `main`.
  It trusts a marker saved on a pull request ref only after checking through the API that the pull
  request goes from `development` to `main` in this repository. Pull request markers from issue
  branches or forks are never trusted, and any API failure counts as a miss.
- Required checks are strict (branches must be up to date), so the promotion pull request tests the
  exact tree that lands on `main`. A job that already passed that tree is skipped there.
- A manual run (`workflow_dispatch`) ignores markers and runs everything, including the nightly
  dispatch.
- Entries expire after 7 days without use; a rerun is then full, which is safe.

`release.yml` calls both `ci.yml` (`checks`, with `release-call: true`) and `e2e.yml` (`e2e`) on the
promotion merge commit. Both skip the heavy jobs whose marker the promotion pull request or the nightly
run already saved for that tree, so the build starts about a minute after the merge. A red end-to-end
suite blocks the release.

Both workflows have a `workflow_call` input `scope` (default `all`; `all`, `shared`, `linux`, `macos` or
`windows`; any other value fails the gate). The gate turns off every job outside the scope: `shared` is
UI, supply chain, release tools, the privacy scan and the text checks; `linux` is `Rust (ubuntu-26.04)` and
`E2E (ubuntu-26.04)`; `macos` is `Rust (macos-latest)`; `windows` is `Rust (windows-latest)` and
`E2E (windows-latest)`. The current calls in `release.yml` pass no scope, so they run the full set minus
the already-passed jobs.

### Release dry run on the promotion pull request

`release.yml` also runs while the promotion pull request is open (opened, reopened, and every push to
`development`). It uses the workflow of the pull request's merge ref, so a release fix on `development`
is exercised before the promotion merges. It builds the merge ref on all three platforms, signs the
updater artifacts with the real key (a same-repository pull request from the PR-protected `development`
branch is the only one that gets it), builds macOS ad-hoc without any Apple secret, runs both
clean-machine smokes, and assembles the assets and `latest.json` into a workflow artifact that expires
after 3 days. It creates no release, draft, tag or attestation, and it skips `checks` and `e2e` (they
run on the same pull request as `ci.yml` and `e2e.yml`). It takes about 25 to 35 minutes per push to the
open promotion pull request; a newer push cancels the older run (concurrency group
`release-dry-run-<PR number>`, separate from the `release` group of real runs).

It is not a required check, and it is not listed under the required checks above: its job names
(`prepare`, `build`, `clean-machine`, ...) are shared with the real release run, and a pull request
from another branch skips them all. Add them to the `Protect main` ruleset only if the owner wants a red
dry run to block the merge. A "Re-run" of a failed `Release` run replays the old merge commit and
workflow, so it is only for transient runner or network failures; a fix goes through `development`
and a new promotion (see G4 in the [release owner checklist](release-owner-checklist.md)).

`ci.yml` and `e2e.yml` no longer run on pushes to `main`: `main` only receives the promotion merge,
which the release call checks. CodeQL and Scorecard still run on `main`.

`Branch policy` (`.github/workflows/branch-policy.yml`) is what restricts the source branch: into
`main` only from `development`, into `development` only from a branch of this repository. A GitHub
ruleset cannot express that, so it is a required check.

## Who can contribute

- Repository setting "pull request creation": collaborators only.
- Workflow runs from outside contributors need owner approval (all external contributors).
- Write access is granted by invitation only.

## Repository settings

- Allow auto-merge: on.
- Automatically delete head branches: on. The rulesets forbid deleting `development` and `main`, so
  the promotion pull request does not remove `development`.
- Dependabot pull requests target `development`.
- Wiki: on, with "Restrict editing to collaborators only" checked. The pages live in `docs/wiki/` and the `Wiki`
  workflow (`.github/workflows/wiki.yml`) mirrors them to the wiki whenever `main` changes them; an
  edit made in the wiki web editor is overwritten by the next sync. GitHub creates the wiki repository
  only when a first page is saved in the web UI, so save one placeholder page once before the first
  sync (or rerun the workflow afterwards from the Actions tab). This is a one-time owner step: until
  the wiki repository exists, the `Wiki` workflow fails with "The wiki repository does not exist yet",
  and it runs again only when `docs/wiki/`, `scripts/ci/wiki_sync.py` or `wiki.yml` changes on `main`, so
  start it by hand (`workflow_dispatch`) after saving the first page.

## Review requirement

The rules require a pull request and green checks, but zero approvals, because a single maintainer
cannot approve their own pull request. When a second maintainer is added, raise the required
approvals to 1 in both rulesets and enable "Require review from Code Owners"
(`.github/CODEOWNERS`).
