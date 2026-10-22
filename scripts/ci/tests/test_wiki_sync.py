"""Unit tests for scripts/ci/wiki_sync.py with temporary source and wiki directories.

Run: py -I -m unittest discover -s scripts/ci/tests -p "test_*.py"
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "wiki_sync.py"
SPEC = importlib.util.spec_from_file_location("wiki_sync", SCRIPT)
assert SPEC and SPEC.loader
wiki_sync = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(wiki_sync)

WIKI_DIR = Path(__file__).resolve().parents[3] / "docs" / "wiki"


class RewriteLinksTests(unittest.TestCase):
    def test_page_links_lose_the_extension_and_keep_the_anchor(self) -> None:
        text = "See [Installation](Installation.md) and [MCP](MCP-Server.md#registering-a-client)."
        self.assertEqual(
            wiki_sync.rewrite_links(text),
            "See [Installation](Installation) and [MCP](MCP-Server#registering-a-client).",
        )

    def test_other_links_are_unchanged(self) -> None:
        text = (
            "[CHANGELOG](https://github.com/example-org/example-repo/blob/main/CHANGELOG.md) "
            "[ops](../ops/release-owner-checklist.md) [site](https://example.org/a.md)"
        )
        self.assertEqual(wiki_sync.rewrite_links(text), text)


class SyncTests(unittest.TestCase):
    def setUp(self) -> None:
        self.scratch = tempfile.TemporaryDirectory()
        root = Path(self.scratch.name)
        self.source = root / "source"
        self.wiki = root / "wiki"
        self.source.mkdir()
        (self.wiki / ".git").mkdir(parents=True)

    def tearDown(self) -> None:
        self.scratch.cleanup()

    def test_writes_pages_and_removes_pages_no_longer_in_the_source(self) -> None:
        (self.source / "Home.md").write_text("# Home\n\n[Start](Getting-Started.md)\n", encoding="utf-8")
        (self.source / "Getting-Started.md").write_text("# Getting Started\n", encoding="utf-8")
        (self.wiki / "Old-Page.md").write_text("# Old\n", encoding="utf-8")
        (self.wiki / "Home.md").write_text("# Stale\n", encoding="utf-8")
        written, removed = wiki_sync.sync(self.source, self.wiki)
        self.assertEqual(written, ["Getting-Started.md", "Home.md"])
        self.assertEqual(removed, ["Old-Page.md"])
        self.assertEqual((self.wiki / "Home.md").read_text(encoding="utf-8"), "# Home\n\n[Start](Getting-Started)\n")
        self.assertTrue((self.wiki / ".git").is_dir())

    def test_an_empty_source_never_empties_the_wiki(self) -> None:
        (self.wiki / "Home.md").write_text("# Home\n", encoding="utf-8")
        with self.assertRaises(SystemExit):
            wiki_sync.sync(self.source, self.wiki)
        self.assertTrue((self.wiki / "Home.md").is_file())

    def test_a_target_that_is_not_a_clone_is_refused(self) -> None:
        (self.source / "Home.md").write_text("# Home\n", encoding="utf-8")
        with self.assertRaises(SystemExit):
            wiki_sync.sync(self.source, self.source)


class RepositoryWikiTests(unittest.TestCase):
    def test_every_page_link_names_an_existing_page(self) -> None:
        pages = {path.name for path in WIKI_DIR.glob("*.md")}
        self.assertIn("Home.md", pages)
        for path in sorted(WIKI_DIR.glob("*.md")):
            for match in wiki_sync.PAGE_LINK_RE.finditer(path.read_text(encoding="utf-8")):
                with self.subTest(page=path.name, link=match.group("page")):
                    self.assertIn(f"{match.group('page')}.md", pages)


if __name__ == "__main__":
    unittest.main()
