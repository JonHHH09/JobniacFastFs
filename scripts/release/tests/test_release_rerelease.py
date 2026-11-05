"""Tests for the re-release version rule of scripts/release/release.py (`next-version --next-patch-even-if-tagged`).

Run from the repository root: `py -I -m unittest discover -s scripts/release/tests -p "test_*.py"`.
All data is synthetic; the git tests build a throwaway repository in a temporary directory.
"""

from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
RELEASE_DIR = HERE.parent
sys.path.insert(0, str(RELEASE_DIR))

import release  # noqa: E402


class ForcedNextPatch(unittest.TestCase):
    def test_the_next_patch_even_when_the_highest_tag_is_on_the_merge_commit(self) -> None:
        # The default treats the tag on the merge commit as a rerun of that version ...
        self.assertEqual(release.next_version("0.1.0", ["v0.1.0", "v0.1.1"], ["v0.1.1"]), ("0.1.1", "v0.1.0"))
        # ... a re-release is a new version of the same commit.
        self.assertEqual(
            release.next_version("0.1.0", ["v0.1.0", "v0.1.1"], ["v0.1.1"], force_next_patch=True), ("0.1.2", "v0.1.1"))

    def test_the_next_patch_when_the_merge_commit_is_untagged(self) -> None:
        self.assertEqual(
            release.next_version("0.1.0", ["v0.1.0", "v0.1.4"], [], force_next_patch=True), ("0.1.5", "v0.1.4"))

    def test_the_patch_is_compared_numerically_and_dev_tags_do_not_count(self) -> None:
        tags = ["v0.1.9", "v0.1.10", "v0.1.2", "v0.2.0-dev.3"]
        self.assertEqual(release.next_version("0.1.0", tags, [], force_next_patch=True), ("0.1.11", "v0.1.10"))
        self.assertEqual(
            release.next_version("0.1.0", tags, ["v0.2.0-dev.3"], force_next_patch=True), ("0.1.11", "v0.1.10"))

    def test_the_files_version_floor_is_ignored(self) -> None:
        # A manual bump to a higher, untagged version sets the floor of a promotion; a re-release of main's
        # commit counts from the last release only.
        self.assertEqual(release.next_version("0.3.0", ["v0.1.4"], [], force_next_patch=True), ("0.1.5", "v0.1.4"))

    def test_without_a_stable_tag_there_is_nothing_to_re_release(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.next_version("0.1.0", [], [], force_next_patch=True)
        with self.assertRaises(release.ReleaseError):
            release.next_version("0.1.0", ["v0.2.0-dev.1"], [], force_next_patch=True)

    def test_the_default_is_unchanged(self) -> None:
        self.assertEqual(release.next_version("0.1.0", ["v0.1.0", "v0.1.1"], ["v0.1.1"]), ("0.1.1", "v0.1.0"))
        self.assertEqual(release.next_version("0.1.0", ["v0.1.0"], []), ("0.1.1", "v0.1.0"))


class RereleaseNotes(unittest.TestCase):
    WINDOWS, LINUX, MACOS = release.PLATFORMS

    def test_a_partial_re_release_says_which_commit_it_builds_again_inside_the_marked_section(self) -> None:
        text = release.platform_notes("1.2.4", (self.MACOS,), None, "example/app", "v1.2.3")
        self.assertTrue(text.startswith(release.PLATFORM_NOTES_BEGIN + "\n"))
        self.assertTrue(text.endswith(release.PLATFORM_NOTES_END + "\n"))
        self.assertIn("This release builds the commit of v1.2.3 again, for macOS (Apple Silicon) only.", text)
        self.assertIn("| Windows (x64) | not available yet |", text)

    def test_a_full_re_release_gets_a_section_with_the_sentence_alone(self) -> None:
        text = release.platform_notes("1.2.4", release.PLATFORMS, None, "example/app", "v1.2.3")
        self.assertIn("This release builds the commit of v1.2.3 again, for all platforms.", text)
        self.assertNotIn("| Platform |", text)

    def test_without_a_re_release_the_notes_are_unchanged(self) -> None:
        self.assertEqual(release.platform_notes("1.2.4", release.PLATFORMS, None, "example/app"), "")
        self.assertNotIn("again", release.platform_notes("1.2.4", (self.LINUX,), None, "example/app"))

    def test_the_section_replaces_a_stale_one_so_a_rerun_never_duplicates_it(self) -> None:
        section = release.platform_notes("1.2.4", (self.MACOS,), None, "example/app", "v1.2.3")
        body = release.apply_platform_section("Notes.\n", section)
        self.assertEqual(release.apply_platform_section(body, section), body)

    def test_a_bad_tag_is_refused(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.platform_notes("1.2.4", (self.MACOS,), None, "example/app", "not a tag")


class PublishedReleaseIsNeverModified(unittest.TestCase):
    """A re-run of failed jobs keeps the old release ID, which may be published by now."""

    def api(self, draft: object, writes: list[str]) -> release.GitHubApi:
        def transport(method: str, url: str, token: str, body: bytes | None, content_type: str) -> tuple[int, bytes]:
            if method != "GET":
                writes.append(f"{method} {url}")
            payload = {"id": 7, "draft": draft, "body": "Notes.\n", "assets": []}
            return 200, json.dumps(payload).encode("utf-8")

        return release.GitHubApi("example/app", "token", transport)

    def test_attach_refuses_a_published_release_and_uploads_nothing(self) -> None:
        directory = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, directory, True)
        (directory / "a.txt").write_bytes(b"a")
        for draft in (False, None, "true"):
            writes: list[str] = []
            with self.subTest(draft=draft), self.assertRaises(release.ReleaseError) as context:
                release.attach_assets(self.api(draft, writes), 7, directory)
            self.assertIn("already published", str(context.exception))
            self.assertEqual(writes, [])

    def test_annotate_refuses_a_published_release_and_patches_nothing(self) -> None:
        writes: list[str] = []
        with self.assertRaises(release.ReleaseError):
            release.annotate_draft(self.api(False, writes), 7, "<!-- platforms -->\nx\n<!-- /platforms -->")
        self.assertEqual(writes, [])

    def test_verify_draft_and_publish_are_no_ops_or_refusals_on_a_published_release(self) -> None:
        writes: list[str] = []
        intent = release.tag_for_version("1.2.3")
        sha = "a" * 40

        def transport(method: str, url: str, token: str, body: bytes | None, content_type: str) -> tuple[int, bytes]:
            if method != "GET":
                writes.append(f"{method} {url}")
            if "/git/ref" in url or "/git/matching-refs" in url:
                return 404, b"{}"
            payload = {"id": 7, "draft": False, "tag_name": "v1.2.3", "prerelease": False,
                       "target_commitish": sha, "assets": []}
            return 200, json.dumps(payload).encode("utf-8")

        api = release.GitHubApi("example/app", "token", transport)
        with self.assertRaises(release.ReleaseError):
            release.publish_release(api, intent, sha, 7)
        self.assertEqual(writes, [])


class ForcedNextPatchGit(unittest.TestCase):
    def run_git(self, root: Path, *arguments: str) -> str:
        completed = release.subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@example.org", "-c", "commit.gpgsign=false",
             "-c", "tag.gpgsign=false", *arguments],
            cwd=root, capture_output=True, encoding="utf-8", check=True,
        )
        return completed.stdout.strip()

    def repository(self) -> tuple[Path, str, str]:
        """A repository with v0.1.0 on the first commit; returns the root, that commit and the tip."""
        if shutil.which("git") is None:
            self.skipTest("git is not installed")
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, True)
        self.run_git(root, "init", "-q")
        self.run_git(root, "commit", "-q", "--allow-empty", "-m", "one")
        first = self.run_git(root, "rev-parse", "HEAD")
        self.run_git(root, "tag", "v0.1.0")
        self.run_git(root, "commit", "-q", "--allow-empty", "-m", "two")
        return root, first, self.run_git(root, "rev-parse", "HEAD")

    def test_the_ancestry_check_still_holds_for_a_tag_on_the_merge_commit(self) -> None:
        root, first, tip = self.repository()
        self.run_git(root, "tag", "v0.1.1")
        tags, at_merge = release.git_release_tags(root, tip)
        self.assertEqual(at_merge, ["v0.1.1"])
        # The tag is on the commit itself, which is its own ancestor: a re-release of the tip is fine.
        release.check_newest_release_is_ancestor(root, tip, tags, at_merge, force_next_patch=True)
        self.assertEqual(release.next_version("0.1.0", tags, at_merge, force_next_patch=True), ("0.1.2", "v0.1.1"))

    def test_an_older_commit_than_the_newest_release_is_refused_even_when_it_is_tagged(self) -> None:
        root, first, tip = self.repository()
        self.run_git(root, "tag", "v0.1.1")
        tags, at_merge = release.git_release_tags(root, first)
        self.assertEqual(at_merge, ["v0.1.0"])
        # The default skips the check for a commit that carries a tag (the rerun path) ...
        release.check_newest_release_is_ancestor(root, first, tags, at_merge)
        # ... a forced next patch is a new release of that commit and must not publish older code.
        with self.assertRaises(release.ReleaseError):
            release.check_newest_release_is_ancestor(root, first, tags, at_merge, force_next_patch=True)

    def test_the_command_prints_the_forced_version(self) -> None:
        root, first, tip = self.repository()
        self.run_git(root, "tag", "v0.1.1")
        parser = release.build_parser()
        args = parser.parse_args(["next-version", "--merge-sha", tip, "--next-patch-even-if-tagged"])
        self.assertTrue(args.next_patch_even_if_tagged)
        self.assertFalse(parser.parse_args(["next-version", "--merge-sha", tip]).next_patch_even_if_tagged)


if __name__ == "__main__":
    unittest.main()
