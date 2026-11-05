"""Unit tests for scripts/ci/gate.sh with synthetic repositories and a stub `gh`.

Each scenario builds a temporary git repository (a base commit and a pull request merge commit whose
first parent is the base), puts a stub `gh` first on PATH and runs the real script together with the
real classifier. The stub answers the two API calls of the gate from files in a directory and ignores
`--jq`: `caches-<key>` holds the refs that the filter would print for the marker key `<key>` (a missing
file makes the call fail), `pull-<n>` holds the `<head ref> <base ref> <head repo>` line that the filter
would print for pull request <n>.

Run: py -I -m unittest discover -s scripts/ci/tests -p "test_*.py"
"""

from __future__ import annotations

import os
import re
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

GATE = Path(__file__).resolve().parents[1] / "gate.sh"
REPO = "example-org/example-repo"

STUB_GH = """#!/usr/bin/env bash
# Stub of `gh api <url> --jq <filter>`: prints the canned answer for the URL, or fails.
url="$2"
case "$url" in
  */actions/caches*)
    key="${url#*key=}"
    key="${key%%&*}"
    file="$GH_STUB_DIR/caches-$key"
    ;;
  */pulls/*) file="$GH_STUB_DIR/pull-${url##*/pulls/}" ;;
  *) exit 1 ;;
esac
[ -f "$file" ] || exit 1
cat "$file"
"""

BASE_FILES = {
    "README.md": "base\n",
    "docs/guide.md": "base\n",
    "src/app.ts": "base\n",
    "crates/core/lib.rs": "base\n",
}

CI_HEAVY = ("ui", "rust-linux", "rust-macos", "rust-windows")
CI_LIGHT = ("supply-chain", "release-tools", "privacy-scan")
CI_JOBS = CI_HEAVY + CI_LIGHT
E2E_JOBS = ("e2e-linux", "e2e-windows")


def git(repo: Path, *args: str) -> str:
    env = dict(os.environ)
    env.update(
        {
            "GIT_AUTHOR_NAME": "Synthetic Author",
            "GIT_AUTHOR_EMAIL": "author@example.com",
            "GIT_COMMITTER_NAME": "Synthetic Author",
            "GIT_COMMITTER_EMAIL": "author@example.com",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
        }
    )
    completed = subprocess.run(["git", *args], cwd=repo, env=env, capture_output=True, text=True, check=True)
    return completed.stdout


def write(repo: Path, relative: str, text: str) -> None:
    path = repo / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


class GateTests(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        self.repo = root / "repo"
        self.repo.mkdir()
        self.stub_dir = root / "stub"
        self.stub_dir.mkdir()
        self.bin_dir = root / "bin"
        self.bin_dir.mkdir()
        gh = self.bin_dir / "gh"
        gh.write_text(STUB_GH, encoding="utf-8")
        gh.chmod(gh.stat().st_mode | stat.S_IXUSR)
        self.output = root / "github-output"
        self.summary = root / "github-summary"
        self.last_stderr = ""

    def make_merge_commit(self, changes: dict[str, str], renames: dict[str, str] | None = None) -> None:
        """Base commit, then a pull request branch merged with --no-ff (first parent = base)."""
        git(self.repo, "init", "-q", "-b", "base")
        for name, text in BASE_FILES.items():
            write(self.repo, name, text)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "base")
        git(self.repo, "checkout", "-q", "-b", "pr")
        for old, new in (renames or {}).items():
            (self.repo / new).parent.mkdir(parents=True, exist_ok=True)
            git(self.repo, "mv", old, new)
        for name, text in changes.items():
            write(self.repo, name, text)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "change")
        git(self.repo, "checkout", "-q", "base")
        git(self.repo, "merge", "-q", "--no-ff", "-m", "merge", "pr")

    def tree(self) -> str:
        return git(self.repo, "rev-parse", "HEAD^{tree}").strip()

    def marker(self, stem: str, *refs: str) -> None:
        """The cache refs that hold the marker `<stem>-<tree>` (no refs: the entry does not exist)."""
        (self.stub_dir / f"caches-{stem}-{self.tree()}").write_text("".join(f"{ref}\n" for ref in refs), encoding="utf-8")

    def pull(self, number: int, head: str, base: str, repo: str = REPO) -> None:
        (self.stub_dir / f"pull-{number}").write_text(f"{head} {base} {repo}\n", encoding="utf-8")

    def gate(
        self,
        workflow: str = "ci",
        event: str = "pull_request",
        base_ref: str = "development",
        head_ref: str = "feature/x",
        head_repo: str = REPO,
        release_call: str = "",
        scope: str = "",
    ) -> subprocess.CompletedProcess[str]:
        env = dict(os.environ)
        env.update(
            {
                "PATH": f"{self.bin_dir}{os.pathsep}{env['PATH']}",
                "PYTHON": sys.executable,
                "GH_STUB_DIR": str(self.stub_dir),
                "GITHUB_OUTPUT": str(self.output),
                "GITHUB_STEP_SUMMARY": str(self.summary),
                "GITHUB_REPOSITORY": REPO,
                "EVENT_NAME": event,
                "BASE_REF": base_ref,
                "HEAD_REF": head_ref,
                "HEAD_REPO": head_repo,
                "RELEASE_CALL": release_call,
                "SCOPE": scope,
            }
        )
        completed = subprocess.run(["bash", str(GATE), workflow], cwd=self.repo, env=env, capture_output=True, text=True)
        self.last_stderr = completed.stderr
        return completed

    def run_gate(self, **kwargs: str) -> dict[str, str]:
        completed = self.gate(**kwargs)
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        pairs = (line.split("=", 1) for line in self.output.read_text(encoding="utf-8").splitlines() if "=" in line)
        return dict(pairs)

    def assertRuns(self, out: dict[str, str], jobs: tuple[str, ...], expected: set[str]) -> None:
        got = {job for job in jobs if out[job] == "true"}
        self.assertEqual(got, expected)

    # --- plain pull requests: path filter ------------------------------------------------------

    def test_plain_pull_request_with_docs_only_runs_only_the_privacy_scan(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        out = self.run_gate()
        self.assertRuns(out, CI_JOBS, {"privacy-scan"})
        self.assertEqual(out["tree"], self.tree())
        self.assertRuns(self.run_gate(workflow="e2e"), E2E_JOBS, set())

    def test_prose_only_change_to_a_fixture_directory_is_not_prose(self) -> None:
        self.make_merge_commit({"crates/cli/tests/fixtures/README.md": "changed\n"})
        out = self.run_gate()
        self.assertRuns(out, CI_JOBS, {"rust-linux", "rust-macos", "rust-windows", "privacy-scan"})

    def test_plain_pull_request_with_rust_change_skips_ui_and_runs_windows(self) -> None:
        self.make_merge_commit({"crates/core/lib.rs": "changed\n"})
        out = self.run_gate()
        self.assertRuns(out, CI_JOBS, {"rust-linux", "rust-macos", "rust-windows", "privacy-scan"})
        self.assertRuns(self.run_gate(workflow="e2e"), E2E_JOBS, set(E2E_JOBS))

    def test_plain_pull_request_with_ui_change_skips_rust(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        out = self.run_gate()
        self.assertRuns(out, CI_JOBS, {"ui", "privacy-scan"})
        self.assertRuns(self.run_gate(workflow="e2e"), E2E_JOBS, set(E2E_JOBS))

    def test_plain_pull_request_with_browser_suite_change_runs_only_the_ui_job(self) -> None:
        self.make_merge_commit({"playwright/shell.spec.ts": "changed\n"})
        self.assertRuns(self.run_gate(), CI_JOBS, {"ui", "privacy-scan"})
        self.assertRuns(self.run_gate(workflow="e2e"), E2E_JOBS, set())

    def test_plain_pull_request_with_changelog_change_runs_the_release_tool_tests(self) -> None:
        self.make_merge_commit({"CHANGELOG.md": "changed\n"})
        self.assertRuns(self.run_gate(), CI_JOBS, {"release-tools", "privacy-scan"})

    def test_plain_pull_request_with_an_unknown_path_runs_everything(self) -> None:
        self.make_merge_commit({"unlisted/file.bin": "changed\n"})
        self.assertRuns(self.run_gate(), CI_JOBS, set(CI_JOBS))
        self.assertRuns(self.run_gate(workflow="e2e"), E2E_JOBS, set(E2E_JOBS))
        self.assertIn("unlisted/file.bin", self.summary.read_text(encoding="utf-8"))

    def test_a_renamed_file_counts_with_its_old_path(self) -> None:
        self.make_merge_commit({}, renames={"src/app.ts": "docs/app.md"})
        self.assertRuns(self.run_gate(), CI_JOBS, {"ui", "privacy-scan"})

    def test_pull_request_into_main_from_another_branch_is_plain(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        out = self.run_gate(base_ref="main", head_ref="dependabot/cargo/x")
        self.assertRuns(out, CI_JOBS, {"privacy-scan"})

    def test_pull_request_from_a_fork_branch_named_development_is_plain(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        out = self.run_gate(base_ref="main", head_ref="development", head_repo="someone-else/example-repo")
        self.assertRuns(out, CI_JOBS, {"privacy-scan"})

    # --- full runs ------------------------------------------------------------------------------

    def test_promotion_pull_request_runs_everything_without_path_filter(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        out = self.run_gate(base_ref="main", head_ref="development")
        self.assertRuns(out, CI_JOBS, set(CI_JOBS))
        self.assertRuns(self.run_gate(workflow="e2e", base_ref="main", head_ref="development"), E2E_JOBS, set(E2E_JOBS))

    def test_release_call_runs_everything(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        for base_ref in ("main", ""):
            out = self.run_gate(base_ref=base_ref, head_ref="development", release_call="true")
            self.assertRuns(out, CI_JOBS, set(CI_JOBS))

    def test_push_and_manual_runs_run_everything(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        for event in ("push", "workflow_dispatch"):
            out = self.run_gate(event=event, base_ref="", head_ref="")
            self.assertRuns(out, CI_JOBS, set(CI_JOBS))

    # --- scope of the release call ------------------------------------------------------------

    def test_scope_turns_jobs_off(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        call = {"base_ref": "main", "head_ref": "development", "release_call": "true"}
        expected = {
            "all": (set(CI_JOBS), set(E2E_JOBS)),
            "": (set(CI_JOBS), set(E2E_JOBS)),
            "shared": ({"ui", "supply-chain", "release-tools", "privacy-scan"}, set()),
            "linux": ({"rust-linux"}, {"e2e-linux"}),
            "macos": ({"rust-macos"}, set()),
            "windows": ({"rust-windows"}, {"e2e-windows"}),
        }
        for scope, (ci, e2e) in expected.items():
            with self.subTest(scope=scope):
                self.assertRuns(self.run_gate(scope=scope, **call), CI_JOBS, ci)
                self.assertRuns(self.run_gate(workflow="e2e", scope=scope, **call), E2E_JOBS, e2e)

    def test_an_invalid_scope_fails_the_gate(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        completed = self.gate(base_ref="main", head_ref="development", release_call="true", scope="everything")
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("invalid scope", completed.stderr)

    # --- markers ----------------------------------------------------------------------------

    def test_a_trusted_marker_skips_only_its_own_job(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-ui", "refs/heads/development")
        out = self.run_gate(event="push", base_ref="", head_ref="")
        self.assertRuns(out, CI_JOBS, set(CI_JOBS) - {"ui"})

    def test_all_markers_skip_all_heavy_jobs_but_not_the_light_ones(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        for stem in ("ci-passed-ui", "ci-passed-rust-linux", "ci-passed-rust-macos", "ci-passed-rust-windows"):
            self.marker(stem, "refs/heads/development")
        out = self.run_gate(event="push", base_ref="", head_ref="")
        self.assertRuns(out, CI_JOBS, set(CI_LIGHT))

    def test_e2e_markers_are_per_platform(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("e2e-passed-windows", "refs/heads/main")
        out = self.run_gate(workflow="e2e", event="push", base_ref="", head_ref="")
        self.assertRuns(out, E2E_JOBS, {"e2e-linux"})

    def test_a_marker_does_not_switch_a_job_on_that_the_classes_leave_off(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.marker("ci-passed-ui", "refs/heads/development")
        self.assertRuns(self.run_gate(), CI_JOBS, {"privacy-scan"})

    def test_main_marker_is_trusted(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-rust-linux", "refs/heads/main")
        out = self.run_gate(base_ref="main", head_ref="development")
        self.assertRuns(out, ("rust-linux",), set())

    def test_marker_of_a_verified_promotion_pull_request_is_trusted(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-ui", "refs/pull/7/merge")
        self.pull(7, "development", "main")
        out = self.run_gate(base_ref="main", head_ref="development", release_call="true")
        self.assertRuns(out, ("ui",), set())

    def test_marker_of_a_pull_request_from_another_branch_is_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-ui", "refs/pull/7/merge")
        self.pull(7, "feature/x", "main")
        out = self.run_gate(base_ref="main", head_ref="development")
        self.assertRuns(out, ("ui",), {"ui"})

    def test_marker_of_a_pull_request_into_development_is_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-ui", "refs/pull/7/merge")
        self.pull(7, "development", "development")
        out = self.run_gate(base_ref="main", head_ref="development")
        self.assertRuns(out, ("ui",), {"ui"})

    def test_marker_of_a_fork_pull_request_is_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-ui", "refs/pull/7/merge")
        self.pull(7, "development", "main", repo="someone-else/example-repo")
        out = self.run_gate(base_ref="main", head_ref="development")
        self.assertRuns(out, ("ui",), {"ui"})

    def test_marker_of_a_pull_request_that_cannot_be_looked_up_is_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-ui", "refs/pull/7/merge")
        out = self.run_gate(base_ref="main", head_ref="development")
        self.assertRuns(out, ("ui",), {"ui"})

    def test_other_cache_refs_are_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-ui", "refs/heads/feature/x", "refs/pull/x/merge", "refs/tags/v1", "refs/pull/7/head")
        self.pull(7, "development", "main")
        out = self.run_gate(base_ref="main", head_ref="development")
        self.assertRuns(out, ("ui",), {"ui"})

    def test_second_entry_can_be_the_trusted_one(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.marker("ci-passed-ui", "refs/pull/8/merge", "refs/pull/7/merge")
        self.pull(8, "feature/x", "main")
        self.pull(7, "development", "main")
        out = self.run_gate(base_ref="main", head_ref="development")
        self.assertRuns(out, ("ui",), set())

    def test_marker_lookup_failure_runs_the_job(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        # No marker files: the stub fails like an API error.
        out = self.run_gate(event="push", base_ref="", head_ref="")
        self.assertRuns(out, CI_JOBS, set(CI_JOBS))
        self.assertIn("passed-marker lookup failed", self.last_stderr)

    def test_lookup_failure_on_a_plain_pull_request_keeps_the_path_filter(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.assertRuns(self.run_gate(), CI_JOBS, {"privacy-scan"})

    def test_manual_run_ignores_markers(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        for stem in ("ci-passed-ui", "ci-passed-rust-linux"):
            self.marker(stem, "refs/heads/development")
        out = self.run_gate(event="workflow_dispatch", base_ref="", head_ref="")
        self.assertRuns(out, CI_JOBS, set(CI_JOBS))

    # --- summary ----------------------------------------------------------------------------

    def test_the_step_summary_names_the_run_the_classes_and_the_reasons(self) -> None:
        self.make_merge_commit({"crates/core/lib.rs": "changed\n"})
        self.marker("ci-passed-rust-macos", "refs/heads/development")
        self.run_gate(base_ref="development")
        self.assertRuns(self.run_gate(event="push", base_ref="", head_ref=""), CI_JOBS, set(CI_JOBS) - {"rust-macos"})
        text = self.summary.read_text(encoding="utf-8")
        self.assertIn("plain pull request", text)
        self.assertIn("Changed files: 1", text)
        self.assertIn("`rust`", text)
        self.assertIn("already passed on this tree", text)
        self.assertIn("no change class needs it", text)

    def test_the_step_summary_names_the_release_call_scope(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.run_gate(base_ref="main", head_ref="development", release_call="true", scope="linux")
        text = self.summary.read_text(encoding="utf-8")
        self.assertRegex(text, re.compile(r"release call \(scope: linux\)"))
        self.assertIn("outside the scope `linux`", text)


if __name__ == "__main__":
    unittest.main()
