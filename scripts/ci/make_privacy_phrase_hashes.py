#!/usr/bin/env python3
"""Write personal phrase fingerprints from a source repository's translation tables (hashes only).

Reads the ``_PHRASES`` tables of the source repository's German and French translation modules with the
``ast`` module (the modules are never imported or executed), normalises every phrase side with the
same rule as ``privacy_scan.py`` and writes SHA-256 digests plus token counts. No phrase text is
written or printed. The output file must be gitignored; the script refuses to write elsewhere.

Usage (stdlib only)::

    py -I scripts/ci/make_privacy_phrase_hashes.py --source-repo <path-to-source-repo>

The source repository is read-only input. Run this on the owner's machine only; CI never has the file.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
import subprocess
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

from privacy_scan import NORMALISATION, REPO_ROOT, normalised_tokens, phrase_digest  # noqa: E402

TABLE_FILES = (
    Path("src/jobniac/application/german_translation.py"),
    Path("src/jobniac/application/french_translation.py"),
)
GENERIC_SOURCES = (
    Path("crates/core/tests/golden/german/translation_pairs.json"),
    Path("crates/core/tests/golden/generation/translation.json"),
)
MIN_TOKENS = 2


def named_pairs(path: Path, name: str) -> list[list[str]]:
    """String sides of every top-level ``name = ((a, b), ...)`` tuple of pairs in ``path``."""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=path.name)
    pairs: list[list[str]] = []
    for node in tree.body:
        if isinstance(node, ast.AnnAssign):
            target, value = node.target, node.value
        elif isinstance(node, ast.Assign) and len(node.targets) == 1:
            target, value = node.targets[0], node.value
        else:
            continue
        if not isinstance(target, ast.Name) or target.id != name or value is None:
            continue
        for pair in value.elts:  # type: ignore[attr-defined]
            sides = [side.value for side in pair.elts if isinstance(side, ast.Constant) and isinstance(side.value, str)]  # type: ignore[attr-defined]
            if sides:
                pairs.append(sides)
    return pairs


def phrase_strings(path: Path) -> list[str]:
    return [side for pair in named_pairs(path, "_PHRASES") for side in pair]


def generic_digests() -> set[str]:
    """Digests of the generic phrases in the tracked golden translation fixtures (never the phrases).

    The ``glossary.phrases`` pairs list neutral job-document terms (role labels, degree names,
    languages) that the German and French tables also translate. They are shared generic terms, not
    personal content, so they are not fingerprinted; their presence in the built-in generic glossary
    is intended.
    """
    digests: set[str] = set()
    for relative in GENERIC_SOURCES:
        path = REPO_ROOT / relative
        if not path.is_file():
            raise SystemExit(f"missing generic phrase fixture: {relative}")
        pairs = json.loads(path.read_text(encoding="utf-8"))["glossary"]["phrases"]
        digests.update(phrase_digest(normalised_tokens(side)) for pair in pairs for side in pair)
    return digests


def is_gitignored(path: Path) -> bool:
    completed = subprocess.run(
        ["git", "check-ignore", "-q", "--", str(path)],
        cwd=REPO_ROOT,
        capture_output=True,
        check=False,
    )
    return completed.returncode == 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--source-repo", required=True, help="read-only checkout of the repository with the translation tables")
    parser.add_argument("--out", default=str(REPO_ROOT / ".privacy-phrase-hashes.json"))
    args = parser.parse_args()

    out = Path(args.out).resolve()
    if not is_gitignored(out):
        print("refusing to write: the output path is not gitignored", file=sys.stderr)
        return 2
    repo = Path(args.source_repo)
    digests: set[str] = set()
    first_digests: set[str] = set()
    counts: dict[int, int] = {}
    skipped_single = 0
    skipped_generic = 0
    total = 0
    generic = generic_digests()
    for relative in TABLE_FILES:
        path = repo / relative
        if not path.is_file():
            print(f"missing table file: {relative.name}", file=sys.stderr)
            return 2
        for text in phrase_strings(path):
            total += 1
            tokens = normalised_tokens(text)
            if len(tokens) < MIN_TOKENS:
                skipped_single += 1
                continue
            digest = phrase_digest(tokens)
            if digest in generic:
                skipped_generic += 1
                continue
            digests.add(digest)
            first_digests.add(hashlib.sha256(tokens[0].encode("utf-8")).hexdigest())
            counts[len(tokens)] = counts.get(len(tokens), 0) + 1
    payload = {
        "version": 1,
        "normalisation": NORMALISATION,
        "min_tokens": MIN_TOKENS,
        "token_counts": sorted(counts),
        "first_token_digests": sorted(first_digests),
        "digests": sorted(digests),
    }
    out.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    print(f"phrase sides read: {total}; fingerprints written: {len(digests)}; "
          f"single-token sides skipped: {skipped_single}; shared generic sides skipped: {skipped_generic}")
    print("token counts (count of fingerprints): " + ", ".join(f"{n}:{counts[n]}" for n in sorted(counts)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
