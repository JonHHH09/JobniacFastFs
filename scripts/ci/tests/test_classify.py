"""Unit tests for scripts/ci/classify.py and its rule file scripts/ci/change_classes.toml.

Run: py -I -m unittest discover -s scripts/ci/tests -p "test_*.py"
"""

from __future__ import annotations

import io
import re
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import classify as cl  # noqa: E402

TABLE = cl.load_table(SCRIPT_DIR / "change_classes.toml")

CI_JOBS = {"ui", "rust-linux", "rust-macos", "rust-windows", "supply-chain", "release-tools", "privacy-scan"}
E2E_JOBS = {"e2e-linux", "e2e-windows"}


def classes_of(path: str) -> set[str]:
    return set(cl.classify_path(TABLE, path))


def running(paths: list[str], workflow: str = "ci", scope: str = "") -> set[str]:
    classes, _ = cl.classify(TABLE, paths)
    jobs = cl.jobs_for(TABLE, classes, workflow, scope)
    return {job for job, (state, _by) in jobs.items() if state == "run"}


class GlobTests(unittest.TestCase):
    def matches(self, glob: str, path: str) -> bool:
        return cl.glob_to_regex(glob).fullmatch(path) is not None

    def test_star_and_question_mark_stay_in_one_segment(self) -> None:
        self.assertTrue(self.matches("*.md", "a.md"))
        self.assertFalse(self.matches("*.md", "dir/a.md"))
        self.assertTrue(self.matches("tsconfig*.json", "tsconfig.node.json"))
        self.assertFalse(self.matches("tsconfig*.json", "tsconfig/x.json"))
        self.assertTrue(self.matches("a?c", "abc"))
        self.assertFalse(self.matches("a?c", "a/c"))

    def test_double_star_crosses_segments(self) -> None:
        self.assertTrue(self.matches("src/**", "src/a/b/c.ts"))
        self.assertFalse(self.matches("src/**", "other/src/a.ts"))
        self.assertTrue(self.matches("**/*.md", "README.md"))
        self.assertTrue(self.matches("**/*.md", "a/b/README.md"))
        self.assertTrue(self.matches("**/golden/**", "crates/core/tests/golden/x/y.json"))
        self.assertTrue(self.matches("**/golden/**", "golden/y.json"))
        self.assertFalse(self.matches("**/golden/**", "crates/goldenrod/y.json"))

    def test_literal_characters_are_escaped(self) -> None:
        self.assertTrue(self.matches("scripts/fetch-pdfium*", "scripts/fetch-pdfium.sh"))
        self.assertFalse(self.matches(".env.mock", "xenvxmock"))
        self.assertTrue(self.matches("a+b", "a+b"))

    def test_double_star_must_be_a_whole_segment(self) -> None:
        with self.assertRaises(cl.RuleError):
            cl.glob_to_regex("src**")
        with self.assertRaises(cl.RuleError):
            cl.glob_to_regex("a**/b")


class RuleTests(unittest.TestCase):
    """One assertion group per rule of change_classes.toml, in the order of the file."""

    def test_fixtures_and_golden_files_are_rust(self) -> None:
        for path in (
            "crates/cli/tests/fixtures/pdf/README.md",
            "crates/cli/tests/fixtures/a.json",
            "crates/core/tests/golden/europass.json",
            "crates/core/tests/golden/notes.md",
        ):
            self.assertEqual(classes_of(path), {"rust"}, path)

    def test_changelog(self) -> None:
        self.assertEqual(classes_of("CHANGELOG.md"), {"changelog"})

    def test_markdown_is_prose(self) -> None:
        for path in ("README.md", "crates/cart/README.md", ".github/PULL_REQUEST_TEMPLATE.md", "e2e/README.md", "src/dev/README.md"):
            self.assertEqual(classes_of(path), {"prose"}, path)

    def test_documentation_and_repository_metadata_are_prose(self) -> None:
        for path in (
            "docs/ops/branch-protection.md",
            "docs/wiki/Home.png",
            "LICENSE",
            ".github/ISSUE_TEMPLATE/bug_report.yml",
            ".github/CODEOWNERS",
            ".github/dependabot.yml",
            ".vscode/extensions.json",
            ".gitignore",
            "osv-scanner.toml",
        ):
            self.assertEqual(classes_of(path), {"prose"}, path)

    def test_gitattributes_is_unknown(self) -> None:
        self.assertEqual(classes_of(".gitattributes"), {"unknown"})

    def test_gate_inputs_and_actions_are_all(self) -> None:
        for path in (
            ".github/actions/rust-checks/action.yml",
            "scripts/ci/gate.sh",
            "scripts/ci/classify.py",
            "scripts/ci/change_classes.toml",
        ):
            self.assertEqual(classes_of(path), {"all"}, path)

    def test_workflow_files(self) -> None:
        self.assertEqual(classes_of(".github/workflows/ci.yml"), {"ci-all"})
        self.assertEqual(classes_of(".github/workflows/e2e.yml"), {"e2e-all"})
        self.assertEqual(classes_of(".github/workflows/release.yml"), {"release"})
        for name in ("nightly", "codeql", "wiki", "branch-policy"):
            self.assertEqual(classes_of(f".github/workflows/{name}.yml"), {"prose"}, name)

    def test_release_files(self) -> None:
        for path in ("scripts/release/release.py", "scripts/release/tests/test_release.py", "src-tauri/tauri.release.conf.json"):
            self.assertEqual(classes_of(path), {"release"}, path)

    def test_other_ci_scripts_are_prose(self) -> None:
        for path in ("scripts/ci/privacy_scan.py", "scripts/ci/wiki_sync.py", "scripts/ci/tests/test_gate.py", "scripts/ci/tests/test_classify.py"):
            self.assertEqual(classes_of(path), {"prose"}, path)

    def test_ipc_is_ui_and_rust(self) -> None:
        self.assertEqual(classes_of("src/ipc/bindings.ts"), {"ui", "rust"})

    def test_ui_files(self) -> None:
        for path in (
            "src/App.tsx",
            "src/components/a/b.tsx",
            "index.html",
            "public/logo.svg",
            "vite.config.ts",
            "tsconfig.json",
            "tsconfig.node.json",
            "biome.json",
            "components.json",
            ".env.mock",
            "pnpm-lock.yaml",
            "pnpm-workspace.yaml",
        ):
            self.assertEqual(classes_of(path), {"ui"}, path)

    def test_package_json_is_ui_and_release(self) -> None:
        self.assertEqual(classes_of("package.json"), {"ui", "release"})

    def test_browser_suite_is_ui_tests(self) -> None:
        self.assertEqual(classes_of("playwright/shell.spec.ts"), {"ui-tests"})
        self.assertEqual(classes_of("playwright.config.ts"), {"ui-tests"})

    def test_e2e_suite_is_e2e_tests(self) -> None:
        self.assertEqual(classes_of("e2e/specs/app.spec.mjs"), {"e2e-tests"})

    def test_cargo_files(self) -> None:
        self.assertEqual(classes_of("Cargo.lock"), {"rust", "supply-chain"})
        self.assertEqual(classes_of("Cargo.toml"), {"rust", "supply-chain", "release"})
        self.assertEqual(classes_of("crates/core/Cargo.toml"), {"rust", "supply-chain"})
        self.assertEqual(classes_of("src-tauri/Cargo.toml"), {"rust", "supply-chain"})
        self.assertEqual(classes_of("deny.toml"), {"supply-chain"})

    def test_tauri_conf_is_rust_and_release(self) -> None:
        self.assertEqual(classes_of("src-tauri/tauri.conf.json"), {"rust", "release"})

    def test_rust_files(self) -> None:
        for path in (
            "crates/core/src/lib.rs",
            "src-tauri/src/lib.rs",
            "src-tauri/tauri.e2e.conf.json",
            "rust-toolchain.toml",
            "clippy.toml",
            ".cargo/config.toml",
            "about.toml",
            "tools/text/generate_digit_table.py",
            "scripts/fetch-pdfium.sh",
            "scripts/fetch-pdfium.ps1",
        ):
            self.assertEqual(classes_of(path), {"rust"}, path)

    def test_a_path_that_matches_no_rule_is_unknown(self) -> None:
        for path in ("Makefile", "scripts/other.sh", ".github/FUNDING.yml", "unlisted/dir/file.txt"):
            self.assertEqual(classes_of(path), {"unknown"}, path)


class OrderingTests(unittest.TestCase):
    """First match wins: the specific rule goes before the general one."""

    def test_fixture_readme_is_rust_but_other_readme_is_prose(self) -> None:
        self.assertEqual(classes_of("crates/cli/tests/fixtures/README.md"), {"rust"})
        self.assertEqual(classes_of("crates/cli/README.md"), {"prose"})

    def test_changelog_is_not_plain_prose(self) -> None:
        self.assertEqual(classes_of("CHANGELOG.md"), {"changelog"})

    def test_ipc_bindings_are_not_plain_ui(self) -> None:
        self.assertEqual(classes_of("src/ipc/bindings.ts"), {"ui", "rust"})

    def test_root_cargo_toml_is_not_a_plain_crate_manifest(self) -> None:
        self.assertEqual(classes_of("Cargo.toml"), {"rust", "supply-chain", "release"})

    def test_ci_scripts_listed_as_all_come_before_the_prose_rule(self) -> None:
        self.assertEqual(classes_of("scripts/ci/gate.sh"), {"all"})
        self.assertEqual(classes_of("scripts/ci/README.md"), {"prose"})

    def test_action_readme_is_prose_because_markdown_comes_first(self) -> None:
        self.assertEqual(classes_of(".github/actions/rust-checks/README.md"), {"prose"})

    def test_the_first_rule_shadows_the_later_ones(self) -> None:
        # Not the union of every matching rule: only the first one counts.
        self.assertEqual(classes_of("src-tauri/tests/fixtures/x.json"), {"rust"})
        self.assertEqual(classes_of("docs/ops/golden/x.md"), {"rust"})


class JobTests(unittest.TestCase):
    def test_an_empty_diff_runs_nothing_conditional(self) -> None:
        self.assertEqual(running([]), {"privacy-scan"})
        self.assertEqual(running([], "e2e"), set())

    def test_prose_only_runs_only_the_privacy_scan(self) -> None:
        self.assertEqual(running(["README.md", "docs/a.md", "LICENSE"]), {"privacy-scan"})
        self.assertEqual(running(["README.md"], "e2e"), set())

    def test_changelog_and_release_run_the_release_tool_tests(self) -> None:
        self.assertEqual(running(["CHANGELOG.md"]), {"release-tools", "privacy-scan"})
        self.assertEqual(running(["scripts/release/release.py"]), {"release-tools", "privacy-scan"})
        self.assertEqual(running(["CHANGELOG.md"], "e2e"), set())

    def test_ui_runs_the_ui_job_and_both_e2e_jobs_but_no_rust(self) -> None:
        self.assertEqual(running(["src/App.tsx"]), {"ui", "privacy-scan"})
        self.assertEqual(running(["src/App.tsx"], "e2e"), E2E_JOBS)

    def test_ui_tests_run_only_the_ui_job(self) -> None:
        self.assertEqual(running(["playwright/shell.spec.ts"]), {"ui", "privacy-scan"})
        self.assertEqual(running(["playwright/shell.spec.ts"], "e2e"), set())

    def test_e2e_tests_run_the_ui_job_and_both_e2e_jobs(self) -> None:
        self.assertEqual(running(["e2e/run.mjs"]), {"ui", "privacy-scan"})
        self.assertEqual(running(["e2e/run.mjs"], "e2e"), E2E_JOBS)

    def test_rust_runs_the_three_rust_jobs_and_both_e2e_jobs(self) -> None:
        self.assertEqual(running(["crates/core/src/lib.rs"]), {"rust-linux", "rust-macos", "rust-windows", "privacy-scan"})
        self.assertEqual(running(["crates/core/src/lib.rs"], "e2e"), E2E_JOBS)

    def test_supply_chain_runs_cargo_deny(self) -> None:
        self.assertEqual(running(["deny.toml"]), {"supply-chain", "privacy-scan"})

    def test_a_crate_manifest_runs_rust_and_cargo_deny(self) -> None:
        self.assertEqual(
            running(["crates/core/Cargo.toml"]),
            {"rust-linux", "rust-macos", "rust-windows", "supply-chain", "privacy-scan"},
        )

    def test_ipc_bindings_run_ui_and_rust(self) -> None:
        self.assertEqual(running(["src/ipc/bindings.ts"]), {"ui", "rust-linux", "rust-macos", "rust-windows", "privacy-scan"})

    def test_classes_union(self) -> None:
        got = running(["README.md", "src/App.tsx", "deny.toml"])
        self.assertEqual(got, {"ui", "supply-chain", "privacy-scan"})

    def test_ci_all_runs_every_ci_job_and_no_e2e_job(self) -> None:
        self.assertEqual(running([".github/workflows/ci.yml"]), CI_JOBS)
        self.assertEqual(running([".github/workflows/ci.yml"], "e2e"), set())

    def test_e2e_all_runs_every_e2e_job_and_only_the_privacy_scan_of_ci(self) -> None:
        self.assertEqual(running([".github/workflows/e2e.yml"], "e2e"), E2E_JOBS)
        self.assertEqual(running([".github/workflows/e2e.yml"]), {"privacy-scan"})

    def test_all_and_unknown_run_everything(self) -> None:
        for path in ("scripts/ci/gate.sh", ".gitattributes", "no/such/file"):
            self.assertEqual(running([path]), CI_JOBS, path)
            self.assertEqual(running([path], "e2e"), E2E_JOBS, path)

    def test_unknown_paths_are_reported(self) -> None:
        classes, unknown = cl.classify(TABLE, ["README.md", "weird.bin", ".gitattributes"])
        self.assertEqual(classes, {"prose", "unknown"})
        self.assertEqual(unknown, ["weird.bin", ".gitattributes"])

    def test_the_reason_lists_the_classes(self) -> None:
        classes, _ = cl.classify(TABLE, ["src/ipc/bindings.ts"])
        jobs = cl.jobs_for(TABLE, classes, "ci", "")
        self.assertEqual(jobs["ui"], ("run", ["ui"]))
        self.assertEqual(jobs["rust-linux"], ("run", ["rust"]))
        self.assertEqual(jobs["supply-chain"], ("no-class", []))
        self.assertEqual(jobs["privacy-scan"], ("run", ["always"]))


class ScopeTests(unittest.TestCase):
    def test_every_scope_filters_the_full_set(self) -> None:
        full = ["scripts/ci/gate.sh"]
        self.assertEqual(running(full, "ci", ""), CI_JOBS)
        self.assertEqual(running(full, "ci", "all"), CI_JOBS)
        self.assertEqual(running(full, "ci", "shared"), {"ui", "supply-chain", "release-tools", "privacy-scan"})
        self.assertEqual(running(full, "ci", "linux"), {"rust-linux"})
        self.assertEqual(running(full, "ci", "macos"), {"rust-macos"})
        self.assertEqual(running(full, "ci", "windows"), {"rust-windows"})
        self.assertEqual(running(full, "e2e", ""), E2E_JOBS)
        self.assertEqual(running(full, "e2e", "all"), E2E_JOBS)
        self.assertEqual(running(full, "e2e", "shared"), set())
        self.assertEqual(running(full, "e2e", "linux"), {"e2e-linux"})
        self.assertEqual(running(full, "e2e", "macos"), set())
        self.assertEqual(running(full, "e2e", "windows"), {"e2e-windows"})

    def test_a_job_outside_the_scope_is_reported_as_such(self) -> None:
        jobs = cl.jobs_for(TABLE, {"all"}, "ci", "linux")
        self.assertEqual(jobs["rust-macos"][0], "out-of-scope")
        self.assertEqual(jobs["rust-linux"][0], "run")

    def test_the_privacy_scan_is_subject_to_the_scope(self) -> None:
        self.assertIn("privacy-scan", running([], "ci", "shared"))
        self.assertNotIn("privacy-scan", running([], "ci", "linux"))

    def test_an_invalid_scope_is_an_error(self) -> None:
        with self.assertRaises(cl.RuleError):
            cl.jobs_for(TABLE, {"all"}, "ci", "everything")
        with self.assertRaises(cl.RuleError):
            cl.jobs_for(TABLE, {"all"}, "ci", "Linux")


class CommandLineTests(unittest.TestCase):
    def run_main(self, argv: list[str], stdin: bytes = b"") -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()

        class Stdin:
            buffer = io.BytesIO(stdin)

        real_stdin = sys.stdin
        sys.stdin = Stdin()  # type: ignore[assignment]
        try:
            with redirect_stdout(out), redirect_stderr(err):
                code = cl.main(argv)
        finally:
            sys.stdin = real_stdin
        return code, out.getvalue(), err.getvalue()

    def test_nul_separated_paths(self) -> None:
        code, out, _ = self.run_main(["--workflow", "ci", "-z"], b"README.md\0src/a b.ts\0")
        self.assertEqual(code, 0)
        self.assertIn("classes prose,ui\n", out)
        self.assertIn("job ui run ui\n", out)
        self.assertIn("job rust-linux no-class -\n", out)

    def test_newline_separated_paths_and_unknown_lines(self) -> None:
        code, out, _ = self.run_main(["--workflow", "e2e"], b"weird.bin\n")
        self.assertEqual(code, 0)
        self.assertIn("unknown weird.bin\n", out)
        self.assertIn("job e2e-windows run unknown\n", out)

    def test_all_flag_ignores_stdin(self) -> None:
        code, out, _ = self.run_main(["--workflow", "ci", "--all", "--scope", "windows"], b"README.md\n")
        self.assertEqual(code, 0)
        self.assertIn("classes all\n", out)
        self.assertIn("job rust-windows run all\n", out)
        self.assertIn("job ui out-of-scope all\n", out)

    def test_unknown_flag_acts_as_one_unmatched_path(self) -> None:
        code, out, _ = self.run_main(["--workflow", "e2e", "--unknown"])
        self.assertEqual(code, 0)
        self.assertIn("classes unknown\n", out)
        self.assertIn("job e2e-linux run unknown\n", out)

    def test_an_invalid_scope_exits_with_2(self) -> None:
        code, _, err = self.run_main(["--workflow", "ci", "--all", "--scope", "nope"])
        self.assertEqual(code, 2)
        self.assertIn("invalid scope", err)

    def test_an_empty_diff(self) -> None:
        code, out, _ = self.run_main(["--workflow", "ci", "-z"], b"")
        self.assertEqual(code, 0)
        self.assertIn("classes -\n", out)


class NewlineTests(unittest.TestCase):
    def test_double_star_crosses_a_newline_in_a_name(self) -> None:
        self.assertEqual(classes_of("docs/a\nb.txt"), {"prose"})
        self.assertEqual(classes_of("docs/a\nb/c.txt"), {"prose"})

    def test_star_matches_a_newline_within_a_segment(self) -> None:
        self.assertEqual(classes_of("notes\nx.md"), {"prose"})


class WorkflowSyncTests(unittest.TestCase):
    """The workflow files, the rule file and gate.sh must name the same jobs.

    A job whose `needs.changes.outputs.<job>` is missing from the gate output would be skipped
    silently (the output is empty), and the aggregator would count that as passed.
    """

    ROOT = SCRIPT_DIR.parents[1]

    def job_blocks(self, workflow: str) -> dict[str, str]:
        text = (self.ROOT / ".github" / "workflows" / f"{workflow}.yml").read_text(encoding="utf-8")
        jobs_text = text.split("\njobs:\n", 1)[1]
        parts = re.split(r"(?m)^  ([A-Za-z0-9_-]+):\n", jobs_text)
        return {parts[i]: parts[i + 1] for i in range(1, len(parts) - 1, 2)}

    def test_gated_jobs_and_referenced_outputs_match_the_rule_file(self) -> None:
        for workflow, expected in TABLE.workflows.items():
            with self.subTest(workflow=workflow):
                blocks = self.job_blocks(workflow)
                gated = set()
                referenced = set()
                for job, block in blocks.items():
                    if_lines = re.findall(r"(?m)^    if: .*$", block)
                    names = set(re.findall(r"needs\.changes\.outputs\.([A-Za-z0-9_-]+)", "\n".join(if_lines)))
                    if names:
                        gated.add(job)
                        referenced |= names
                self.assertEqual(gated, set(expected))
                self.assertEqual(referenced, set(expected))
                declared = set(re.findall(r"(?m)^      ([A-Za-z0-9_-]+): \$\{\{ steps\.gate\.outputs\.\1 \}\}$", blocks["changes"]))
                self.assertEqual(declared, set(expected) | {"tree"})

    def test_gate_script_expects_the_same_jobs(self) -> None:
        text = (SCRIPT_DIR / "gate.sh").read_text(encoding="utf-8")
        for workflow, expected in TABLE.workflows.items():
            with self.subTest(workflow=workflow):
                match = re.search(rf'(?m)^  {workflow}\)\n    expected_jobs="([^"]*)"', text)
                self.assertIsNotNone(match)
                self.assertEqual(set(match.group(1).split()), set(expected))  # type: ignore[union-attr]


class RuleFileTests(unittest.TestCase):
    def write_rules(self, text: str) -> Path:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path = Path(tmp.name) / "rules.toml"
        path.write_text(text, encoding="utf-8")
        return path

    def test_a_rule_with_an_undefined_class_is_rejected(self) -> None:
        base = (SCRIPT_DIR / "change_classes.toml").read_text(encoding="utf-8")
        path = self.write_rules(base + '\n[[rule]]\nglobs = ["x"]\nclasses = ["nope"]\n')
        with self.assertRaises(cl.RuleError):
            cl.load_table(path)

    def test_a_class_with_an_undefined_job_is_rejected(self) -> None:
        base = (SCRIPT_DIR / "change_classes.toml").read_text(encoding="utf-8")
        path = self.write_rules(base.replace('supply-chain = ["supply-chain"]', 'supply-chain = ["no-such-job"]'))
        with self.assertRaises(cl.RuleError):
            cl.load_table(path)

    def test_the_real_rule_file_loads(self) -> None:
        self.assertGreater(len(TABLE.rules), 10)


if __name__ == "__main__":
    unittest.main()
