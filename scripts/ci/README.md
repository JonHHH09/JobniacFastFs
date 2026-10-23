# CI helper scripts

## Privacy scan (`privacy_scan.py`)

Scans the working tree (tracked files and untracked files that are not ignored) and every object
reachable from any ref (`git rev-list --all`, remote-tracking refs included) for private data and
secrets. It reports the commits and paths that add each flagged blob.

Run from the repository root (Windows: `py -I`; Linux: `python3 -I`):

    py -I scripts/ci/privacy_scan.py                 # working tree and history, generic classes
    py -I scripts/ci/privacy_scan.py --mode worktree # working tree only
    py -I scripts/ci/privacy_scan.py --report <path> # full JSON report (exact paths and commits)

Exit status: `0` no finding after the allow-list, `1` findings, `2` usage or configuration error.
Output shows each match masked; `--report` writes exact paths and commit ids and never matched values.
Keep reports outside the repository.

### Inputs

- `privacy_patterns.json`: generic classes (local paths, e-mail addresses, IP addresses, tokens and
  keys, database URLs with passwords, phone and street shapes, date-of-birth lines, binary documents by
  magic bytes). No owner-specific value is stored here.
- `privacy_allowlist.json`: reviewed known-synthetic or intentionally public hits. Each entry has a
  content SHA-256, the classes it covers and a reason. A changed file has a new digest and is reviewed
  again. Entries without a reason or with a malformed digest stop the scan (exit 2).
- `.privacy-local.json` (gitignored, owner machine only): owner-specific literals (user name, machine
  name, personal e-mail, employer names). The environment variable `JOBNIAC_PRIVACY_LOCAL_JSON` may
  carry the same JSON instead. Without this input the owner classes are not checked; the scan says so.
- `.privacy-phrase-hashes.json` (gitignored, owner machine only): SHA-256 fingerprints of normalised
  phrases from private German and French translation tables. Phrase text is never stored or printed.
  Write it with the generator below. CI does not have this file.
- `--require-local` makes the run fail when either owner-side input is missing (use it for the owner run).

### Phrase fingerprints (`make_privacy_phrase_hashes.py`)

    py -I scripts/ci/make_privacy_phrase_hashes.py --source-repo <path-to-source-repo>

The script reads the `_PHRASES` tables with `ast` (nothing is imported or executed), drops every side
that also appears in the generic phrase sets of the golden translation fixtures
(`crates/core/tests/golden/german/translation_pairs.json` and
`crates/core/tests/golden/generation/translation.json`), and writes digests
and token counts only. It refuses to write to a path that git does not ignore. Single-token sides are
skipped (a single common word would match everywhere); owner-specific single words belong in
`.privacy-local.json`.

### Tests

    py -I -m unittest discover -s scripts/ci/tests -p "test_*.py"

The tests build their own repositories and probe strings at run time, so the test sources contain no
path, secret or personal value.

### Allow-list review

Add an entry only after reading the flagged content. Do not add entries for personal data, real
credentials or real contact details: those need a fix in the content or a history rewrite decided by
the owner. The CI job `privacy-scan` runs the unit tests and the generic scan on every push and pull
request.

## Workflow helpers

### apt drop-in (`.github/actions/apt-config`)

The Linux jobs of `rust-checks`, `e2e-suite` and the UI job of `ci.yml` run this small composite action
before their first `apt-get update`. It writes `/etc/apt/apt.conf.d/99-ci-network` with `Acquire::Retries "3"`
and a 20 second `http` and `https` timeout, so a stalled download from the runner image mirror is retried
instead of waiting for minutes. The drop-in also covers the apt calls of `playwright install --with-deps`.
dpkg is never wrapped in `timeout`. `release.yml` does not use it.

### Nightly alert (`.github/workflows/nightly-alert.yml`)

`nightly.yml` only dispatches `ci.yml` and `e2e.yml` on development and is green itself. The alert workflow
runs on `workflow_run` of those two workflows and, when a dispatched run on development ends as `failure`,
`cancelled` or `timed_out`, comments on the open issue labelled `ci-nightly` (creating the label and the
issue when needed) with the run URL, workflow, commit and failed jobs. A manual dispatch on development
alerts too. It checks out nothing and only reads run metadata. `workflow_run` uses the copy of the file on
the default branch, so it works once promoted to main.
