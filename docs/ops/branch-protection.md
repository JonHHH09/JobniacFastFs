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
  `release.yml`, so the owner merges it deliberately.

## Rulesets

| Ruleset | Branch | Rules |
|---|---|---|
| Protect development | `development` | no deletion, no force push, pull request required (squash only, conversations resolved), required status checks, branches must be up to date |
| Protect main | `main` | the same, merge commit only |

There is no bypass actor: direct pushes are blocked for everyone, including the owner.

Required status checks (named as the jobs are named in the workflows): `Branch policy`, `UI`,
`Rust (ubuntu-22.04)`, `Supply chain (cargo-deny)`, `Release tool unit tests` and
`Privacy scan (working tree and history, generic classes)`.

`Rust (windows-latest)` and `E2E (...)` are not required yet: they fail on `main` today (three
`platform_windows` export tests in `jobniac-infra`, and the end-to-end suite on both systems), so
requiring them would block every pull request, including the fix. Add them to both rulesets once
they are green.

### Skipped checks and the passed marker

A required check whose job is skipped counts as passed, so `ci.yml` and `e2e.yml` skip work safely:

- A `Changes` job (`scripts/ci/gate.sh`) path-filters pull requests: a docs-only change skips `UI`,
  `Rust (...)` and `E2E (...)`; Rust-only changes skip `UI`.
- `Rust (windows-latest)` and `E2E (windows-latest)` do not run on pull requests, only on pushes to
  `development` and `main`, on manual runs (`workflow_dispatch`) and, for Rust, on the release call.
- Every job that is a required check must keep a literal `name:`. Do not turn the Rust or E2E jobs
  back into a matrix: a skipped matrix job reports its unexpanded name, and the required check would
  stay pending forever.
- The gate fails open: every heavy job runs when the `Changes` job did not succeed
  (`needs.changes.result != 'success'`), because a job skipped behind a failed gate would otherwise
  count as passed. `PostgreSQL 18 integration` keeps its push and release-call restriction in that case.
- After a fully green push run, `Mark tree as passed` saves a cache entry `ci-passed-<tree SHA>` (or
  `e2e-passed-<tree SHA>`). The gate skips the heavy jobs for a tree whose entry exists and was saved
  by a push to `development` or `main` (pull request caches are never trusted). The promotion pull
  request, the push to `main` and the release `checks` call therefore skip when the tree is the one
  that passed on `development`. Entries expire after 7 days without use; a rerun is then full, which is
  safe.

`PostgreSQL 18 integration` is not listed: it is opt-in on pull requests, so it does not report
there. It runs on every push to `development` and `main`, and `release.yml` requires it on the
promotion merge commit.

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
