#!/usr/bin/env bash
# Gate of the CI and E2E workflows: decides which heavy jobs run and writes the decision to
# $GITHUB_OUTPUT. Usage: gate.sh <ci|e2e>
#
# Inputs (environment):
#   EVENT_NAME         github.event_name (the caller's event for a workflow_call)
#   POSTGRES_REQUIRED  "true" for the release call of ci.yml: a full run, no path filtering
#   GH_TOKEN           token with `actions: read`, used to look the passed marker up
#   GITHUB_REPOSITORY, GITHUB_OUTPUT  set by the runner
#
# Outputs:
#   tree            git tree SHA of the checked-out commit (the identity of the tested content)
#   already-passed  true when a trusted push already passed this workflow on this exact tree
#   rust, ui, app   true when the Rust, UI or desktop-app jobs have to run (already-passed folded in)
#   windows         true when the Windows jobs have to run (never on a plain pull request)
#
# A marker is the Actions cache entry `<workflow>-passed-<tree>` that the `mark-passed` job saves at
# the end of a fully green push run. It is trusted only when it was saved by a push to development or
# main: caches written by pull requests (forks included) are scoped to the pull request ref and ignored.
set -euo pipefail

workflow="${1:?usage: gate.sh <ci|e2e>}"
event="${EVENT_NAME:?}"
required="${POSTGRES_REQUIRED:-false}"

tree="$(git rev-parse 'HEAD^{tree}')"
echo "tree=$tree" >> "$GITHUB_OUTPUT"

# A plain pull request is path-filtered and skips Windows. Pushes, manual runs and the release call
# (a pull_request event with postgres-required) run the full set.
plain_pr=false
if [ "$event" = "pull_request" ] && [ "$required" != "true" ]; then
  plain_pr=true
fi

# 1. Has this exact tree already passed on a trusted push? A manual run always runs everything.
already=false
if [ "$event" != "workflow_dispatch" ]; then
  key="${workflow}-passed-${tree}"
  hits="$(gh api "repos/${GITHUB_REPOSITORY}/actions/caches?key=${key}&per_page=100" \
    --jq "[.actions_caches[] | select(.key == \"${key}\" and (.ref == \"refs/heads/development\" or .ref == \"refs/heads/main\"))] | length" \
    || { echo "::warning::passed-marker lookup failed; running all checks" >&2; echo 0; })"
  if [ "${hits:-0}" -gt 0 ]; then
    already=true
  fi
fi
echo "already-passed=$already" >> "$GITHUB_OUTPUT"

# 2. Path filter (plain pull requests only). The checkout of a pull request is the merge commit, so
#    its first parent is the base branch tip and `HEAD^1..HEAD` is what the pull request changes.
rust=true ui=true app=true
if [ "$plain_pr" = true ]; then
  changed="$(git diff --name-only HEAD^1 HEAD)"
  echo "Changed files:"
  echo "$changed"

  matches() { # matches <extended regex>
    grep -Eq "$1" <<< "$changed"
  }

  # The UI dist is embedded by tauri::generate_context!, so UI sources count for the Rust jobs.
  ui_paths='^(src/|e2e/|tools/|public/|package\.json$|pnpm-lock\.yaml$|pnpm-workspace\.yaml$|index\.html$|vite\.config\.ts$|tsconfig[^/]*\.json$|biome\.json$|components\.json$|\.env\.mock$)'
  rust_paths='^(crates/|src-tauri/|Cargo\.(toml|lock)$|rust-toolchain|clippy\.toml$|deny\.toml$|about\.toml$|\.cargo/|\.gitattributes$|\.github/actions/|scripts/fetch-pdfium|scripts/ci/install-postgresql-18\.sh$)'

  rust=false ui=false app=false
  # The workflow file, its composite actions and this script affect every job of the workflow.
  if matches "^(\.github/actions/|scripts/ci/gate\.sh$|\.github/workflows/${workflow}\.yml$)"; then
    rust=true ui=true app=true
  fi
  if matches "$ui_paths"; then rust=true ui=true app=true; fi
  # Rust-only changes cannot break the UI lint, type-check and unit tests: that job stays off.
  if matches "$rust_paths"; then rust=true app=true; fi
fi

# 3. Fold already-passed in: nothing heavy runs for a tree that already passed.
if [ "$already" = true ]; then
  rust=false ui=false app=false
fi

windows=false
if [ "$plain_pr" != true ]; then
  windows=true
fi
if [ "$already" = true ]; then
  windows=false
fi

{
  echo "rust=$rust"
  echo "ui=$ui"
  echo "app=$app"
  echo "windows=$windows"
} >> "$GITHUB_OUTPUT"
echo "gate($workflow): event=$event already-passed=$already rust=$rust ui=$ui app=$app windows=$windows"
