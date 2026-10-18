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
  leaves the draft unpublished.

## Rulesets

| Ruleset | Branch | Rules |
|---|---|---|
| Protect development | `development` | no deletion, no force push, pull request required (squash only, conversations resolved), required status checks, branches must be up to date |
| Protect main | `main` | the same, merge commit only |

There is no bypass actor: direct pushes are blocked for everyone, including the owner.

Required status checks (named as the jobs are named in the workflows): `Branch policy`, `UI`,
`Rust (ubuntu-26.04)`, `Supply chain (cargo-deny)`, `Release tool unit tests` and
`Privacy scan (working tree and history, generic classes)`.

Linux jobs run on the latest Ubuntu LTS runner (`ubuntu-26.04`). The one exception is the Linux leg
of the release build in `release.yml`, which stays on `ubuntu-24.04` because the build host sets the
glibc floor of the AppImage and deb. When the runner moves to a new LTS, the `Rust (ubuntu-...)`
check is renamed, so update the required check in both rulesets in the same change.

`Rust (windows-latest)` and `E2E (...)` are not required checks: they do not run on pull requests
into `development`. `E2E (windows-latest)` still fails on `main`. The end-to-end suite is a release
gate (`release.yml` needs it), so a promotion pull request whose `E2E (...)` checks are red must not
be merged: the release would stop after the merge.

### Skipped checks and the passed marker

A required check whose job is skipped counts as passed, so `ci.yml` and `e2e.yml` skip work safely:

- The `Changes` job (`scripts/ci/gate.sh`) path-filters pull requests into `development`: a
  docs-only change skips `UI`, `Rust (...)` and `E2E (...)`; Rust-only changes skip `UI`.
- `Rust (windows-latest)` and `E2E (windows-latest)` do not run on pull
  requests into `development`. They run on the promotion pull request (`development` to `main`), on
  the nightly run, on manual runs (`workflow_dispatch`) and in the release call.
- Every job that is a required check must keep a literal `name:`. Do not turn the Rust or E2E jobs
  back into a matrix: a skipped matrix job reports its unexpanded name, and the required check would
  stay pending forever.
- The gate fails open: every heavy job runs when the `Changes` job did not succeed
  (`needs.changes.result != 'success'`), because a job skipped behind a failed gate would otherwise
  count as passed.
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
- After a fully green run, `Mark tree as passed` saves a cache entry `ci-passed-<tree SHA>` (or
  `e2e-passed-<tree SHA>`). It is saved by a push or a manual or nightly run on `development` or
  `main`, and by the promotion pull request.
- The gate trusts a marker saved on `development` or `main`. It trusts a marker saved on a pull
  request ref only after checking through the API that the pull request goes from `development` to
  `main` in this repository. Pull request markers from issue branches or forks are never trusted.
- Required checks are strict (branches must be up to date), so the promotion pull request tests the
  exact tree that lands on `main`. When that tree already passed, the gate skips the heavy jobs there.
- A manual run (`workflow_dispatch`) ignores the marker and runs everything, including the nightly
  dispatch.
- Entries expire after 7 days without use; a rerun is then full, which is safe.

`release.yml` calls both `ci.yml` (`checks`, with `release-call: true`) and `e2e.yml` (`e2e`) on the
promotion merge commit. Both skip the heavy jobs when the promotion pull request or the nightly run
already passed that tree, so the build starts about a minute after the merge. A red end-to-end suite
blocks the release.

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

## Review requirement

The rules require a pull request and green checks, but zero approvals, because a single maintainer
cannot approve their own pull request. When a second maintainer is added, raise the required
approvals to 1 in both rulesets and enable "Require review from Code Owners"
(`.github/CODEOWNERS`).
