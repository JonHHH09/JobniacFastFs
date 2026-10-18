"""Unit tests for scripts/ci/gate.sh with synthetic repositories and a stub `gh`.

Each scenario builds a temporary git repository (a base commit and a pull request merge commit whose
first parent is the base), puts a stub `gh` first on PATH and runs the real script. The stub answers
the two API calls of the gate from files in a directory and ignores `--jq`: `caches` holds the refs
that the filter would print for the marker key (a missing file makes the call fail), `pull-<n>` holds
the `<head ref> <base ref> <head repo>` line that the filter would print for pull request <n>.

Run: py -I -m unittest discover -s scripts/ci/tests -p "test_*.py"
"""

from __future__ import annotations

import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

GATE = Path(__file__).resolve().parents[1] / "gate.sh"
REPO = "example-org/example-repo"

STUB_GH = """#!/usr/bin/env bash
# Stub of `gh api <url> --jq <filter>`: prints the canned answer for the URL, or fails.
url="$2"
case "$url" in
  */actions/caches*) file="$GH_STUB_DIR/caches" ;;
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

    def make_merge_commit(self, changes: dict[str, str]) -> None:
        """Base commit, then a pull request branch merged with --no-ff (first parent = base)."""
        git(self.repo, "init", "-q", "-b", "base")
        for name, text in BASE_FILES.items():
            write(self.repo, name, text)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "base")
        git(self.repo, "checkout", "-q", "-b", "pr")
        for name, text in changes.items():
            write(self.repo, name, text)
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "change")
        git(self.repo, "checkout", "-q", "base")
        git(self.repo, "merge", "-q", "--no-ff", "-m", "merge", "pr")

    def tree(self) -> str:
        return git(self.repo, "rev-parse", "HEAD^{tree}").strip()

    def caches(self, *refs: str) -> None:
        (self.stub_dir / "caches").write_text("".join(f"{ref}\n" for ref in refs), encoding="utf-8")

    def pull(self, number: int, head: str, base: str, repo: str = REPO) -> None:
        (self.stub_dir / f"pull-{number}").write_text(f"{head} {base} {repo}\n", encoding="utf-8")

    def run_gate(self, workflow: str = "ci", event: str = "pull_request", base_ref: str = "development", required: str = "") -> dict[str, str]:
        env = dict(os.environ)
        env.update(
            {
                "PATH": f"{self.bin_dir}{os.pathsep}{env['PATH']}",
                "GH_STUB_DIR": str(self.stub_dir),
                "GITHUB_OUTPUT": str(self.output),
                "GITHUB_REPOSITORY": REPO,
                "EVENT_NAME": event,
                "BASE_REF": base_ref,
                "RELEASE_CALL": required,
            }
        )
        completed = subprocess.run(["bash", str(GATE), workflow], cwd=self.repo, env=env, capture_output=True, text=True)
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        self.last_stderr = completed.stderr
        pairs = (line.split("=", 1) for line in self.output.read_text(encoding="utf-8").splitlines() if "=" in line)
        return dict(pairs)

    def assertDecision(self, out: dict[str, str], *, rust: bool, ui: bool, app: bool, windows: bool, already: bool) -> None:
        got = {key: out[key] == "true" for key in ("rust", "ui", "app", "windows")}
        got["already-passed"] = out["already-passed"] == "true"
        want = {"rust": rust, "ui": ui, "app": app, "windows": windows, "already-passed": already}
        self.assertEqual(got, want)

    def test_plain_pull_request_with_docs_only_runs_nothing(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.caches()
        out = self.run_gate()
        self.assertDecision(out, rust=False, ui=False, app=False, windows=False, already=False)
        self.assertEqual(out["tree"], self.tree())

    def test_plain_pull_request_with_rust_change_skips_ui(self) -> None:
        self.make_merge_commit({"crates/core/lib.rs": "changed\n"})
        self.caches()
        out = self.run_gate()
        self.assertDecision(out, rust=True, ui=False, app=True, windows=False, already=False)

    def test_plain_pull_request_with_ui_change_runs_everything_but_windows(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches()
        out = self.run_gate()
        self.assertDecision(out, rust=True, ui=True, app=True, windows=False, already=False)

    def test_promotion_pull_request_runs_everything_without_path_filter(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.caches()
        out = self.run_gate(base_ref="main")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_release_call_runs_everything(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.caches()
        out = self.run_gate(base_ref="main", required="true")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_release_call_without_base_ref_runs_everything(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.caches()
        out = self.run_gate(base_ref="", required="true")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_e2e_workflow_uses_the_same_decision(self) -> None:
        self.make_merge_commit({"crates/core/lib.rs": "changed\n"})
        self.caches()
        out = self.run_gate(workflow="e2e", base_ref="main")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_push_with_development_marker_is_already_passed(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches("refs/heads/development")
        out = self.run_gate(event="push", base_ref="")
        self.assertDecision(out, rust=False, ui=False, app=False, windows=False, already=True)

    def test_main_marker_is_trusted(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches("refs/heads/main")
        out = self.run_gate(base_ref="main")
        self.assertDecision(out, rust=False, ui=False, app=False, windows=False, already=True)

    def test_push_without_marker_runs_everything(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.caches()
        out = self.run_gate(event="push", base_ref="")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_marker_of_a_verified_promotion_pull_request_is_trusted(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches("refs/pull/7/merge")
        self.pull(7, "development", "main")
        out = self.run_gate(base_ref="main", required="true")
        self.assertDecision(out, rust=False, ui=False, app=False, windows=False, already=True)

    def test_marker_of_a_pull_request_from_another_branch_is_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches("refs/pull/7/merge")
        self.pull(7, "feature/x", "main")
        out = self.run_gate(base_ref="main")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_marker_of_a_pull_request_into_development_is_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches("refs/pull/7/merge")
        self.pull(7, "development", "development")
        out = self.run_gate(base_ref="main")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_marker_of_a_fork_pull_request_is_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches("refs/pull/7/merge")
        self.pull(7, "development", "main", repo="someone-else/example-repo")
        out = self.run_gate(base_ref="main")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_marker_of_a_pull_request_that_cannot_be_looked_up_is_ignored(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches("refs/pull/7/merge")
        out = self.run_gate(base_ref="main")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_other_cache_refs_are_ignored(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.caches("refs/heads/feature/x", "refs/pull/x/merge", "refs/tags/v1", "refs/pull/7/head")
        self.pull(7, "development", "main")
        out = self.run_gate(base_ref="main")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)

    def test_second_entry_can_be_the_trusted_one(self) -> None:
        self.make_merge_commit({"src/app.ts": "changed\n"})
        self.caches("refs/pull/8/merge", "refs/pull/7/merge")
        self.pull(8, "feature/x", "main")
        self.pull(7, "development", "main")
        out = self.run_gate(base_ref="main")
        self.assertDecision(out, rust=False, ui=False, app=False, windows=False, already=True)

    def test_marker_lookup_failure_runs_everything(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        # No `caches` file: the stub fails like an API error.
        out = self.run_gate(event="push", base_ref="")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)
        self.assertIn("passed-marker lookup failed", self.last_stderr)

    def test_lookup_failure_on_a_plain_pull_request_keeps_the_path_filter(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        out = self.run_gate()
        self.assertDecision(out, rust=False, ui=False, app=False, windows=False, already=False)

    def test_manual_run_ignores_the_marker(self) -> None:
        self.make_merge_commit({"docs/guide.md": "changed\n"})
        self.caches("refs/heads/development")
        out = self.run_gate(event="workflow_dispatch", base_ref="")
        self.assertDecision(out, rust=True, ui=True, app=True, windows=True, already=False)


if __name__ == "__main__":
    unittest.main()
