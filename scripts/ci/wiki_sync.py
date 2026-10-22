"""Mirror docs/wiki/ into a clone of the GitHub wiki (stdlib only).

The wiki pages are written and reviewed in the repository under docs/wiki/; the `Wiki` workflow runs
this script against a fresh clone of `<repo>.wiki.git` and pushes the result. The wiki is a read-only
mirror: a page that is no longer in docs/wiki/ is removed from it.

Links between pages are written as `Page.md` so they also work when browsing the repository. The
GitHub wiki addresses a page without the extension, so a relative link to a `.md` page loses it here.

Run: py -I scripts/ci/wiki_sync.py --source docs/wiki --wiki <wiki clone>
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# A Markdown link target that names a page of the wiki: no scheme, no slash, ends in .md, optional anchor.
PAGE_LINK_RE = re.compile(r"\]\((?P<page>[A-Za-z0-9_-]+)\.md(?P<anchor>#[^)\s]*)?\)")


def rewrite_links(text: str) -> str:
    return PAGE_LINK_RE.sub(lambda match: f"]({match.group('page')}{match.group('anchor') or ''})", text)


def sync(source: Path, wiki: Path) -> tuple[list[str], list[str]]:
    """Writes every source page into the wiki and removes the wiki pages it no longer has.

    Returns the written and the removed page names. Fails on an empty source, so a wrong path never
    empties the wiki.
    """
    pages = sorted(path for path in source.glob("*.md") if path.is_file())
    if not pages:
        raise SystemExit(f"FAIL: no .md page in {source}")
    if not (wiki / ".git").exists():
        raise SystemExit(f"FAIL: {wiki} is not a git clone of the wiki")
    names = {page.name for page in pages}
    removed = []
    for existing in sorted(wiki.glob("*.md")):
        if existing.name not in names:
            existing.unlink()
            removed.append(existing.name)
    for page in pages:
        text = page.read_text(encoding="utf-8")
        (wiki / page.name).write_bytes(rewrite_links(text).encode("utf-8"))
    return [page.name for page in pages], removed


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Mirror docs/wiki/ into a clone of the GitHub wiki")
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--wiki", type=Path, required=True)
    args = parser.parse_args(argv)
    written, removed = sync(args.source, args.wiki)
    sys.stdout.write(f"wiki: {len(written)} pages written, {len(removed)} removed\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
