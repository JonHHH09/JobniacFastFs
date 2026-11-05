#!/usr/bin/env bash
# Gate of the CI and E2E workflows: decides which jobs run and writes the decision to $GITHUB_OUTPUT
# and a short Markdown summary to $GITHUB_STEP_SUMMARY. Usage: gate.sh <ci|e2e>
#
# Inputs (environment):
#   EVENT_NAME         github.event_name (the caller's event for a workflow_call)
#   BASE_REF           github.base_ref (the target branch of a pull request, empty otherwise)
#   HEAD_REF           github.head_ref (the source branch of a pull request, empty otherwise)
#   HEAD_REPO          github.event.pull_request.head.repo.full_name (empty outside pull requests)
#   RELEASE_CALL       "true" for the release call of ci.yml and e2e.yml
#   SCOPE              the `scope` input of the release call: all, shared, linux, macos or windows;
#                      empty means all, anything else fails the gate
#   GH_TOKEN           token with `actions: read` and `pull-requests: read`, used to look markers up
#   PYTHON             interpreter of the classifier (default python3; run locally with PYTHON=python)
#   GITHUB_REPOSITORY, GITHUB_OUTPUT, GITHUB_STEP_SUMMARY  set by the runner
#
# Outputs:
#   tree        git tree SHA of the checked-out commit (the identity of the tested content)
#   <job id>    true when that job has to run, one output per job of the workflow (ci: ui, rust-linux,
#               rust-macos, rust-windows, supply-chain, release-tools, privacy-scan, text; e2e: e2e-linux,
#               e2e-windows). A job runs when its change class is on, it is in the scope and, for the
#               jobs that have a marker, no trusted marker says this tree already passed it.
#
# Which runs are filtered: only a plain pull request (neither the release call nor a promotion pull
# request) is path-filtered, by the change classes of scripts/ci/change_classes.toml (scripts/ci/classify.py).
# A promotion pull request (development into main, same repository), a push, a manual run and the release
# call run the full set, minus the jobs whose marker is trusted (a manual run ignores markers).
#
# A marker is the Actions cache entry `<workflow>-passed-<job>-<tree>` that the `mark-passed` job saves for
# each heavy job that passed on development or main (a push or the nightly dispatch) or on a promotion
# pull request. It is trusted when its cache ref is refs/heads/development or refs/heads/main, or
# refs/pull/<n>/merge of a pull request that the API shows to be a promotion pull request of this
# repository. Caches written by any other pull request (forks included) are scoped to their own ref and
# ignored. Required checks are strict (the branch must be up to date), so the merge commit tree of a
# promotion pull request equals the tree that lands on main. Any API failure is a miss.
set -euo pipefail

workflow="${1:?usage: gate.sh <ci|e2e>}"
event="${EVENT_NAME:?}"
release_call="${RELEASE_CALL:-false}"
base_ref="${BASE_REF:-}"
head_ref="${HEAD_REF:-}"
head_repo="${HEAD_REPO:-}"
scope="${SCOPE:-}"
repository="${GITHUB_REPOSITORY:?}"
summary="${GITHUB_STEP_SUMMARY:-/dev/null}"
python="${PYTHON:-python3}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$workflow" in
  # expected_jobs: every job id the workflow gates (the [workflows] table of change_classes.toml and the
  # `needs.changes.outputs.<job>` conditions of the workflow file; test_classify.py keeps the three in sync).
  ci)
    expected_jobs="ui rust-linux rust-macos rust-windows supply-chain release-tools privacy-scan text"
    marked_jobs="ui rust-linux rust-macos rust-windows"
    ;;
  e2e)
    expected_jobs="e2e-linux e2e-windows"
    marked_jobs="e2e-linux e2e-windows"
    ;;
  *) echo "::error::usage: gate.sh <ci|e2e>" >&2; exit 1 ;;
esac

tree="$(git rev-parse 'HEAD^{tree}')"
echo "tree=$tree" >> "$GITHUB_OUTPUT"

# 1. Kind of run. Only a plain pull request is path-filtered.
if [ "$release_call" = "true" ]; then
  kind="release call (scope: ${scope:-all})"
  plain_pr=false
elif [ "$event" = "pull_request" ]; then
  if [ "$base_ref" = "main" ] && [ "$head_ref" = "development" ] && [ "$head_repo" = "$repository" ]; then
    kind="promotion pull request"
    plain_pr=false
  else
    kind="plain pull request"
    plain_pr=true
  fi
else
  case "$event" in
    push) kind="push" ;;
    workflow_dispatch) kind="manual run (workflow_dispatch)" ;;
    *) kind="$event" ;;
  esac
  plain_pr=false
fi

# 2. Classes and the jobs they switch on. The checkout of a pull request is the merge commit, so its
#    first parent is the base branch tip and `HEAD^1..HEAD` is what the pull request changes.
if [ "$plain_pr" = true ] && ! git rev-parse --verify --quiet 'HEAD^2' > /dev/null; then
  # Not a merge commit: HEAD^1 is not known to be the base tip, so the diff cannot be trusted.
  changed_count="not counted (the checkout is not a merge commit: treated as unknown, full run)"
  echo "::warning::the checkout is not a pull request merge commit; running everything" >&2
  classified="$("$python" -I "$script_dir/classify.py" --workflow "$workflow" --scope "$scope" --unknown)" \
    || { echo "::error::the change classifier failed" >&2; exit 1; }
elif [ "$plain_pr" = true ]; then
  # --no-renames lists the old path of a rename too.
  changed_nul="$(mktemp)"
  trap 'rm -f "$changed_nul"' EXIT
  git diff --name-only --no-renames -z HEAD^1 HEAD > "$changed_nul"
  changed_count="$(tr -cd '\0' < "$changed_nul" | wc -c | tr -d ' ')"
  echo "Changed files: $changed_count"
  # Indented, so that no name can start a line with `::` (a workflow command).
  tr '\0' '\n' < "$changed_nul" | sed 's/^/  /'
  classified="$("$python" -I "$script_dir/classify.py" --workflow "$workflow" --scope "$scope" -z < "$changed_nul")" \
    || { echo "::error::the change classifier failed" >&2; exit 1; }
else
  changed_count="not counted (full run, no path filter)"
  classified="$("$python" -I "$script_dir/classify.py" --workflow "$workflow" --scope "$scope" --all)" \
    || { echo "::error::the change classifier failed (invalid scope?)" >&2; exit 1; }
fi

classes=""
unknown_paths=()
declare -A state by
jobs=()
while IFS= read -r line; do
  case "$line" in
    "classes "*) classes="${line#classes }" ;;
    "unknown "*) unknown_paths+=("${line#unknown }") ;;
    "job "*)
      read -r _ name job_state job_by <<< "$line"
      jobs+=("$name")
      state[$name]="$job_state"
      by[$name]="$job_by"
      ;;
  esac
done <<< "$classified"

# The workflows read one output per job: the classifier must emit exactly the expected jobs, or a job
# would be skipped silently behind an empty output.
if [ "$(printf '%s\n' "${jobs[@]}" | sort | tr '\n' ' ')" != "$(printf '%s\n' $expected_jobs | sort | tr '\n' ' ')" ]; then
  echo "::error::the classifier emitted jobs [${jobs[*]}] but the gate expects [$expected_jobs]" >&2
  exit 1
fi

# 3. Markers. A job that the classes and the scope switch on is skipped when a trusted marker says
#    this tree already passed it. A manual run ignores markers.
promotion_pr() { # promotion_pr <number>: a development -> main pull request of this repository?
  local info
  info="$(gh api "repos/${repository}/pulls/$1" \
    --jq '[.head.ref, .base.ref, .head.repo.full_name] | join(" ")' 2> /dev/null)" || return 1
  [ "$info" = "development main ${repository}" ]
}

lookup_failed=false
marker_trusted() { # marker_trusted <key>: is there a trusted cache entry with exactly this key?
  local key="$1" refs ref number
  if ! refs="$(gh api "repos/${repository}/actions/caches?key=${key}&per_page=100" \
    --jq ".actions_caches[] | select(.key == \"${key}\") | .ref")"; then
    lookup_failed=true
    return 1
  fi
  while IFS= read -r ref; do
    case "$ref" in
      refs/heads/development | refs/heads/main) return 0 ;;
      refs/pull/*/merge)
        number="${ref#refs/pull/}"
        number="${number%/merge}"
        if [[ "$number" =~ ^[0-9]+$ ]] && promotion_pr "$number"; then
          return 0
        fi
        ;;
    esac
  done <<< "$refs"
  return 1
}

declare -A run why
for name in "${jobs[@]}"; do
  run[$name]=false
  case "${state[$name]}" in
    out-of-scope) why[$name]="skipped: outside the scope \`${scope:-all}\`" ;;
    no-class) why[$name]="skipped: no change class needs it" ;;
    run)
      why[$name]="runs: ${by[$name]}"
      case " $marked_jobs " in
        *" $name "*)
          if [ "$event" != "workflow_dispatch" ]; then
            if marker_trusted "${workflow}-passed-${name#"${workflow}"-}-${tree}"; then
              why[$name]="skipped: already passed on this tree (trusted marker)"
              continue
            fi
          fi
          ;;
      esac
      run[$name]=true
      ;;
    *)
      echo "::error::the classifier gave job $name the unknown state '${state[$name]}'" >&2
      exit 1
      ;;
  esac
done
if [ "$lookup_failed" = true ]; then
  echo "::warning::passed-marker lookup failed; the affected jobs run" >&2
fi

# 4. Outputs and summary.
{
  for name in "${jobs[@]}"; do
    echo "$name=${run[$name]}"
  done
} >> "$GITHUB_OUTPUT"

{
  echo "### Gate ($workflow)"
  echo
  echo "- Run: $kind"
  echo "- Changed files: $changed_count"
  echo "- Change classes: \`${classes}\`"
  if [ "${#unknown_paths[@]}" -gt 0 ]; then
    echo "- Unknown paths (no rule matched, so every job is on):"
    for path in "${unknown_paths[@]}"; do
      # Control characters, backticks and pipes would break the Markdown (or inject lines into it).
      safe="${path//[[:cntrl:]]/ }"
      safe="${safe//\`/}"
      safe="${safe//|/}"
      echo "  - \`${safe}\`"
    done
  fi
  echo
  echo "| Job | Runs | Why |"
  echo "|---|---|---|"
  for name in "${jobs[@]}"; do
    echo "| $name | ${run[$name]} | ${why[$name]} |"
  done
} >> "$summary"

line="gate($workflow): $kind classes=$classes"
for name in "${jobs[@]}"; do
  line="$line $name=${run[$name]}"
done
echo "$line"
