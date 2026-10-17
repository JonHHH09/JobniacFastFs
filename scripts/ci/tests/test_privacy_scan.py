"""Unit tests for scripts/ci/privacy_scan.py with synthetic inputs created at run time.

No test literal contains a path, secret or personal value in one piece: probes are assembled from
fragments so that this file itself passes the scan. Repositories are created in temporary
directories with a neutral git identity.

Run: py -I -m unittest discover -s scripts/ci/tests -p "test_*.py"
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPT_DIR))

import privacy_scan as ps  # noqa: E402

SCAN = SCRIPT_DIR / "privacy_scan.py"
PATTERNS = SCRIPT_DIR / "privacy_patterns.json"

WIN_PROFILE = "C" + ":" + "\\" + "Users" + "\\" + "casey-tester" + "\\" + "Projects"
POSIX_HOME = "/" + "home" + "/" + "casey-tester" + "/" + "work"
GITHUB_TOKEN = "gh" + "p" + "_" + "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"
PRIVATE_KEY = "-----" + "BEGIN " + "RSA PRIVATE KEY" + "-----"
PERSON_EMAIL = "dev" + "@" + "corp-sample" + ".net"
PUBLIC_IP = "203" + ".0.113.7"
PRIVATE_IP = "10" + ".20.30.40"


def make_scanner(literals: list[str] | None = None, phrases: ps.PhraseIndex = ps.EMPTY_PHRASES) -> ps.ContentScanner:
    rules, settings, validators = ps.load_generic_patterns(PATTERNS)
    config = ps.ScanConfig(
        rules=rules,
        settings=settings,
        owner_rules=ps.owner_rules(literals or []),
        phrases=phrases,
    )
    return ps.ContentScanner(config, validators)


def classes_of(scanner: ps.ContentScanner, text: str, path: str | None = None) -> set[str]:
    result = scanner.scan([text.encode("utf-8")], path)
    return {hit.cls for hit in result.hits}


def git(repo: Path, *args: str, env: dict[str, str] | None = None) -> str:
    full_env = dict(os.environ)
    full_env.update(
        {
            "GIT_AUTHOR_NAME": "Synthetic Author",
            "GIT_AUTHOR_EMAIL": "author@example.com",
            "GIT_COMMITTER_NAME": "Synthetic Author",
            "GIT_COMMITTER_EMAIL": "author@example.com",
            "GIT_CONFIG_NOSYSTEM": "1",
        }
    )
    if env:
        full_env.update(env)
    completed = subprocess.run(["git", *args], cwd=repo, env=full_env, capture_output=True, text=True, check=True)
    return completed.stdout


class MaskTests(unittest.TestCase):
    def test_short_values_are_fully_masked(self) -> None:
        self.assertEqual(ps.mask("abc"), "***")

    def test_long_values_keep_only_the_ends(self) -> None:
        masked = ps.mask("abcdefghijkl")
        self.assertEqual(masked, "ab********kl")
        self.assertNotIn("cdefghij", masked)


class GenericClassTests(unittest.TestCase):
    def setUp(self) -> None:
        self.scanner = make_scanner()

    def test_windows_profile_path_is_flagged(self) -> None:
        self.assertIn("local-path", classes_of(self.scanner, f"see {WIN_PROFILE}"))

    def test_escaped_windows_profile_path_is_flagged(self) -> None:
        escaped = WIN_PROFILE.replace("\\", "\\\\")
        self.assertIn("local-path", classes_of(self.scanner, escaped))

    def test_posix_home_path_is_flagged(self) -> None:
        self.assertIn("local-path", classes_of(self.scanner, f"cwd {POSIX_HOME}"))

    def test_placeholders_and_ci_runner_paths_are_not_flagged(self) -> None:
        text = "C:\\Users\\<USER>\\x and /home/runner/work and C:\\Users\\Public\\x"
        self.assertNotIn("local-path", classes_of(self.scanner, text))

    def test_example_and_noreply_emails_are_allowed(self) -> None:
        text = "a@example.com b@users.noreply.github.com git@github.com:org/repo.git x@y.test"
        self.assertNotIn("email", classes_of(self.scanner, text))

    def test_other_emails_are_flagged(self) -> None:
        self.assertIn("email", classes_of(self.scanner, f"contact {PERSON_EMAIL}"))

    def test_documentation_and_loopback_addresses_are_allowed(self) -> None:
        self.assertNotIn("ip-address", classes_of(self.scanner, f"{PUBLIC_IP} 127.0.0.1 0.0.0.0"))

    def test_private_addresses_are_flagged(self) -> None:
        self.assertIn("ip-address", classes_of(self.scanner, f"host {PRIVATE_IP}"))

    def test_github_token_is_flagged(self) -> None:
        self.assertIn("secret-token", classes_of(self.scanner, f"token={GITHUB_TOKEN}"))

    def test_private_key_block_is_flagged(self) -> None:
        self.assertIn("private-key", classes_of(self.scanner, PRIVATE_KEY))

    def test_database_url_with_real_password_is_flagged(self) -> None:
        url = "postgres" + "://" + "appuser" + ":" + "Kx9pQ2wRt7" + "@" + "db.internal.example"
        self.assertIn("db-credentials", classes_of(self.scanner, url))

    def test_database_url_placeholder_is_not_flagged(self) -> None:
        url = "postgres://<USER>:<PASSWORD>@<HOST>:<PORT>/db"
        self.assertNotIn("db-credentials", classes_of(self.scanner, url))

    def test_secret_assignment_with_placeholder_is_not_flagged(self) -> None:
        self.assertNotIn("secret-token", classes_of(self.scanner, 'api_key = "synthetic-placeholder-value-123"'))

    def test_hex_digests_are_not_secret_assignments(self) -> None:
        digest = hashlib.sha256(b"x").hexdigest()
        self.assertNotIn("secret-token", classes_of(self.scanner, f'token = "{digest}"'))

    def test_password_in_config_file_is_flagged_only_in_config_paths(self) -> None:
        text = 'password = "Sup3rS3cretValue"'
        self.assertIn("db-credentials", classes_of(self.scanner, text, "config/app.toml"))
        self.assertNotIn("db-credentials", classes_of(self.scanner, text, "src/lib.rs"))

    def test_street_and_phone_shapes_are_flagged(self) -> None:
        street = "12 " + "rue" + " Example"
        phone = "+" + "33 6 12 34 56 78"
        self.assertIn("personal-data", classes_of(self.scanner, street))
        self.assertIn("personal-data", classes_of(self.scanner, phone))

    def test_binary_document_magic_is_flagged(self) -> None:
        result = self.scanner.scan([b"%PDF-1.4\n" + b"x" * 50], "docs/a.pdf")
        self.assertEqual(result.binary_kind, "pdf")
        self.assertIn("binary-document", {hit.cls for hit in result.hits})

    def test_plain_text_is_not_a_binary_document(self) -> None:
        result = self.scanner.scan([b"plain text\n"], "docs/a.txt")
        self.assertIsNone(result.binary_kind)


class OwnerAndPhraseTests(unittest.TestCase):
    def test_owner_literal_is_matched_case_insensitively_on_word_boundaries(self) -> None:
        scanner = make_scanner(literals=["synthetic-owner"])
        self.assertIn("owner-identity", classes_of(scanner, "Hello SYNTHETIC-OWNER, bye"))
        self.assertNotIn("owner-identity", classes_of(scanner, "xsynthetic-ownerx"))

    def test_phrase_fingerprint_matches_normalised_phrase(self) -> None:
        digest = ps.phrase_digest(ps.normalised_tokens("Synthetic Alpha, beta"))
        index = ps.PhraseIndex(
            digests=frozenset([digest]),
            first_digests=frozenset([hashlib.sha256(b"synthetic").hexdigest()]),
            lengths=(3,),
            count=1,
        )
        scanner = make_scanner(phrases=index)
        self.assertIn("personal-phrase", classes_of(scanner, "A line with SYNTHETIC alpha   BETA here"))
        self.assertNotIn("personal-phrase", classes_of(scanner, "A line with other words"))

    def test_phrase_hit_reports_line_number(self) -> None:
        digest = ps.phrase_digest(ps.normalised_tokens("synthetic alpha beta"))
        index = ps.PhraseIndex(frozenset([digest]), frozenset([hashlib.sha256(b"synthetic").hexdigest()]), (3,), 1)
        scanner = make_scanner(phrases=index)
        result = scanner.scan([b"one\ntwo\nsynthetic alpha beta\n"], None)
        lines = [hit.line for hit in result.hits if hit.cls == "personal-phrase"]
        self.assertEqual(lines, [3])


class StreamingTests(unittest.TestCase):
    def test_match_across_chunk_boundary_is_found_once(self) -> None:
        scanner = make_scanner()
        token = GITHUB_TOKEN.encode("ascii")
        filler = b"a" * (ps.CHUNK_BYTES - 11) + b" "
        data = filler + token + b"\n" + b"z" * 100
        chunks = [data[: ps.CHUNK_BYTES - 5], data[ps.CHUNK_BYTES - 5 :]]
        result = scanner.scan(chunks, None)
        tokens = [hit for hit in result.hits if hit.cls == "secret-token"]
        self.assertEqual(len(tokens), 1)
        self.assertEqual(tokens[0].line, 1)

    def test_line_numbers_survive_chunking(self) -> None:
        scanner = make_scanner()
        head = (b"x" * 100 + b"\n") * 20000  # about 2 MB, many lines
        data = head + PERSON_EMAIL.encode("ascii") + b"\n"
        size = len(data)
        chunks = [data[i : i + ps.CHUNK_BYTES] for i in range(0, size, ps.CHUNK_BYTES)]
        result = scanner.scan(chunks, None)
        emails = [hit for hit in result.hits if hit.cls == "email"]
        self.assertEqual(len(emails), 1)
        self.assertEqual(emails[0].line, 20001)


class AllowlistTests(unittest.TestCase):
    def test_entry_without_reason_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "allow.json"
            path.write_text(
                json.dumps({"version": 1, "entries": [{"sha256": "a" * 64, "classes": ["email"], "reason": ""}]}),
                encoding="utf-8",
            )
            with self.assertRaises(ps.ConfigError):
                ps.load_allowlist(path)

    def test_entry_with_bad_digest_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "allow.json"
            path.write_text(
                json.dumps({"version": 1, "entries": [{"sha256": "short", "classes": ["email"], "reason": "r"}]}),
                encoding="utf-8",
            )
            with self.assertRaises(ps.ConfigError):
                ps.load_allowlist(path)

    def test_record_suppresses_only_matching_content_and_class(self) -> None:
        digest = "b" * 64
        allow = {digest: {"email": "reviewed synthetic sample"}}
        report = ps.Report()
        ps.record(report, allow, ps.Occurrence("worktree", "f", "email", "r", 1, "x", digest, None, [], []))
        ps.record(report, allow, ps.Occurrence("worktree", "f", "ip-address", "r", 1, "x", digest, None, [], []))
        ps.record(report, allow, ps.Occurrence("worktree", "f", "email", "r", 1, "x", "c" * 64, None, [], []))
        self.assertEqual(report.suppressed, {"email": 1})
        self.assertEqual([o.cls for o in report.occurrences], ["ip-address", "email"])


class RepositoryTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "user.name", "Synthetic Author")
        git(self.repo, "config", "user.email", "author@example.com")

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def _commit(self, message: str, env: dict[str, str] | None = None) -> str:
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", message, env=env)
        return git(self.repo, "rev-parse", "HEAD").strip()

    def _scan(self, mode: str) -> ps.Report:
        rules, settings, validators = ps.load_generic_patterns(PATTERNS)
        config = ps.ScanConfig(rules=rules, settings=settings)
        return ps.run_scan(self.repo, mode, config, validators, {}, progress=False)

    def test_secret_deleted_in_later_commit_is_found_in_history_only(self) -> None:
        (self.repo / "notes.txt").write_text(f"contact {PERSON_EMAIL}\n", encoding="utf-8")
        leak_commit = self._commit("add notes")
        (self.repo / "notes.txt").unlink()
        self._commit("remove notes")

        worktree = self._scan("worktree")
        self.assertFalse([o for o in worktree.occurrences if o.cls == "email"])

        history = self._scan("history")
        emails = [o for o in history.occurrences if o.cls == "email" and o.source == "history"]
        self.assertEqual(len(emails), 1)
        self.assertIn(leak_commit, emails[0].commits)
        self.assertIn("notes.txt", emails[0].paths)

    def test_author_email_in_commit_metadata_is_found(self) -> None:
        self._commit("empty", env={"GIT_AUTHOR_EMAIL": PERSON_EMAIL, "GIT_COMMITTER_EMAIL": PERSON_EMAIL})
        history = self._scan("history")
        metadata = [o for o in history.occurrences if o.source == "metadata" and o.cls == "email"]
        self.assertTrue(metadata)

    def test_structural_ids_do_not_trigger_phone_patterns(self) -> None:
        (self.repo / "a.txt").write_text("plain\n", encoding="utf-8")
        self._commit("plain")
        history = self._scan("history")
        self.assertFalse([o for o in history.occurrences if o.cls == "personal-data"])

    def test_branch_only_in_remote_tracking_ref_is_scanned(self) -> None:
        (self.repo / "b.txt").write_text(f"x {PERSON_EMAIL}\n", encoding="utf-8")
        commit = self._commit("on branch")
        git(self.repo, "update-ref", "refs/remotes/origin/feature", commit)
        git(self.repo, "checkout", "-q", "--detach")
        git(self.repo, "update-ref", "-d", "refs/heads/main")
        history = self._scan("history")
        self.assertTrue([o for o in history.occurrences if o.cls == "email" and o.source == "history"])


class CommandLineTests(unittest.TestCase):
    def _run(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-I", str(SCAN), *args],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_clean_repository_exits_zero(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init", "-q", "-b", "main")
            (repo / "ok.txt").write_text("nothing here\n", encoding="utf-8")
            git(repo, "add", "-A")
            git(repo, "commit", "-q", "-m", "ok")
            completed = self._run("--repo", str(repo), "--mode", "all", "--local", str(repo / "absent.json"),
                                  "--phrases", str(repo / "absent-phrases.json"),
                                  "--allowlist", str(SCRIPT_DIR / "privacy_allowlist.json"))
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)

    def test_leak_exits_one_and_masks_the_value(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git(repo, "init", "-q", "-b", "main")
            (repo / "leak.txt").write_text(f"{PERSON_EMAIL}\n", encoding="utf-8")
            git(repo, "add", "-A")
            git(repo, "commit", "-q", "-m", "leak")
            completed = self._run("--repo", str(repo), "--mode", "worktree", "--local", str(repo / "absent.json"),
                                  "--phrases", str(repo / "absent-phrases.json"),
                                  "--allowlist", str(SCRIPT_DIR / "privacy_allowlist.json"))
            self.assertEqual(completed.returncode, 1)
            self.assertNotIn(PERSON_EMAIL, completed.stdout)
            self.assertIn("[email]", completed.stdout)

    def test_invalid_allowlist_exits_two(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "allow.json"
            bad.write_text(json.dumps({"version": 1, "entries": [{"sha256": "x"}]}), encoding="utf-8")
            completed = self._run("--mode", "worktree", "--local", str(Path(tmp) / "absent.json"),
                                  "--phrases", str(Path(tmp) / "absent.json"), "--allowlist", str(bad))
            self.assertEqual(completed.returncode, 2)


class PhraseGeneratorTests(unittest.TestCase):
    def test_refuses_output_outside_gitignore(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            completed = subprocess.run(
                [sys.executable, "-I", str(SCRIPT_DIR / "make_privacy_phrase_hashes.py"),
                 "--source-repo", tmp, "--out", str(Path(tmp) / "hashes.json")],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(completed.returncode, 2)


if __name__ == "__main__":
    unittest.main()
