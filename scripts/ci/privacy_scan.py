#!/usr/bin/env python3
"""Privacy pattern scan of the working tree and of the full git history.

Stdlib only. Windows: ``py -I scripts/ci/privacy_scan.py``. Linux CI: ``python3 -I scripts/ci/privacy_scan.py``.

What is scanned:

- the working tree: tracked files and untracked files that are not ignored;
- the history: every blob, commit and tag object reachable from any ref (``git rev-list --all``
  plus every ref tip, so remote-tracking refs count), with each blob attributed to the commits and
  paths that add it;
- names: working-tree paths, history paths and ref names.

Generic classes come from ``privacy_patterns.json``. Owner-specific literals (user name, machine
name, employer names, personal e-mail) come from a gitignored local file (``.privacy-local.json``,
or the JSON text in the environment variable ``JOBNIAC_PRIVACY_LOCAL_JSON``) and are never printed.
Personal phrase fingerprints (SHA-256 of normalised phrases, never the phrases) come from
``.privacy-phrase-hashes.json``, written by ``make_privacy_phrase_hashes.py``. Reviewed known-synthetic
hits are listed in ``privacy_allowlist.json`` by content SHA-256 and class, so a changed file
triggers review again.

Output never contains a matched value in full: every match is shown masked. The optional JSON report
(``--report``) holds exact paths and commit ids so that a history rewrite can be planned; keep it
outside the repository.

Exit status: 0 no findings after the allow-list, 1 findings, 2 usage or configuration error.
"""

from __future__ import annotations

import argparse
import bisect
import hashlib
import ipaddress
import json
import os
import re
import subprocess
import sys
import threading
import unicodedata
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass, field
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[1]
DEFAULT_PATTERNS = SCRIPT_DIR / "privacy_patterns.json"
DEFAULT_ALLOWLIST = SCRIPT_DIR / "privacy_allowlist.json"
DEFAULT_LOCAL = REPO_ROOT / ".privacy-local.json"
DEFAULT_PHRASES = REPO_ROOT / ".privacy-phrase-hashes.json"

CHUNK_BYTES = 1 << 20
OVERLAP_BYTES = 16 << 10
TOKEN_RE = re.compile(r"[^\W_]+")
OFFICE_EXTENSIONS = {".docx", ".xlsx", ".pptx", ".odt", ".ods", ".odp", ".zip"}
NORMALISATION = (
    "unicodedata NFC, then casefold; tokens are maximal runs of Unicode letters and digits "
    "(underscore excluded); a phrase digest is SHA-256 of its tokens joined by one ASCII space, UTF-8"
)
DOCUMENTATION_NETWORKS = (
    ipaddress.ip_network("192.0.2.0/24"),
    ipaddress.ip_network("198.51.100.0/24"),
    ipaddress.ip_network("203.0.113.0/24"),
)


class ConfigError(Exception):
    """A usage, configuration or git error that stops the scan with exit status 2."""


def normalised_tokens(text: str) -> list[str]:
    """Tokens of ``text`` as used for phrase fingerprints (shared with the hash generator)."""
    return TOKEN_RE.findall(unicodedata.normalize("NFC", text).casefold())


def phrase_digest(tokens: Iterable[str]) -> str:
    return hashlib.sha256(" ".join(tokens).encode("utf-8")).hexdigest()


def mask(value: str) -> str:
    """Masked form of a matched value: enough to find it, never the whole value."""
    n = len(value)
    if n <= 6:
        return "*" * n
    return value[:2] + "*" * (n - 4) + value[-2:]


def ascii_safe(text: str) -> str:
    return text.encode("ascii", "backslashreplace").decode("ascii")


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class PhraseIndex:
    digests: frozenset[str]
    first_digests: frozenset[str]
    lengths: tuple[int, ...]
    count: int


EMPTY_PHRASES = PhraseIndex(frozenset(), frozenset(), (), 0)


@dataclass(frozen=True)
class Rule:
    rule_id: str
    cls: str
    regex: re.Pattern[str]
    group: int
    validator: str | None
    path_regex: re.Pattern[str] | None


@dataclass
class Settings:
    placeholder_markers: tuple[str, ...]
    allowed_email_domains: frozenset[str]
    allowed_email_suffixes: tuple[str, ...]
    allowed_emails: frozenset[str]
    generic_profile_names: frozenset[str]


@dataclass
class ScanConfig:
    rules: list[Rule]
    settings: Settings
    owner_rules: list[Rule] = field(default_factory=list)
    phrases: PhraseIndex = EMPTY_PHRASES
    owner_literal_count: int = 0


def _validator_table(settings: Settings) -> dict[str, Callable[[str], bool]]:
    markers = settings.placeholder_markers

    def is_placeholder(value: str) -> bool:
        lowered = value.lower()
        return any(marker.lower() in lowered for marker in markers) or value.startswith("$")

    def not_placeholder(value: str) -> bool:
        return not is_placeholder(value)

    def not_placeholder_or_hex_digest(value: str) -> bool:
        if is_placeholder(value):
            return False
        return not (len(value) >= 32 and re.fullmatch(r"[0-9a-fA-F]+", value))

    def email_not_allowed(value: str) -> bool:
        email = value.lower()
        if email in settings.allowed_emails or is_placeholder(email):
            return False
        if re.search(r"@[0-9]", email):  # retina icon names such as name@2x.png, not addresses
            return False
        domain = email.rsplit("@", 1)[-1]
        if any(domain == d or domain.endswith("." + d) for d in settings.allowed_email_domains):
            return False
        if domain.endswith(settings.allowed_email_suffixes):
            return False
        return True

    def profile_name_not_generic(value: str) -> bool:
        return value.lower() not in settings.generic_profile_names and not is_placeholder(value)

    def ip_not_reserved(value: str) -> bool:
        address = ipaddress.ip_address(value)
        if address.is_loopback or address.is_unspecified or address == ipaddress.ip_address("255.255.255.255"):
            return False
        return not any(address in network for network in DOCUMENTATION_NETWORKS)

    def phone_digit_count(value: str) -> bool:
        return 9 <= sum(ch.isdigit() for ch in value) <= 15

    return {
        "not_placeholder": not_placeholder,
        "not_placeholder_or_hex_digest": not_placeholder_or_hex_digest,
        "email_not_allowed": email_not_allowed,
        "profile_name_not_generic": profile_name_not_generic,
        "ip_not_reserved": ip_not_reserved,
        "phone_digit_count": phone_digit_count,
    }


def load_generic_patterns(path: Path) -> tuple[list[Rule], Settings, dict[str, Callable[[str], bool]]]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ConfigError(f"cannot read pattern file {path.name}: {exc}") from exc
    if data.get("version") != 1:
        raise ConfigError(f"unsupported pattern file version in {path.name}")
    settings = Settings(
        placeholder_markers=tuple(data["placeholder_markers"]),
        allowed_email_domains=frozenset(d.lower() for d in data["allowed_email_domains"]),
        allowed_email_suffixes=tuple(s.lower() for s in data["allowed_email_suffixes"]),
        allowed_emails=frozenset(e.lower() for e in data["allowed_emails"]),
        generic_profile_names=frozenset(n.lower() for n in data["generic_profile_names"]),
    )
    validators = _validator_table(settings)
    rules: list[Rule] = []
    for entry in data["rules"]:
        flags = re.IGNORECASE if entry.get("ignorecase") else 0
        validator = entry.get("validator")
        if validator is not None and validator not in validators:
            raise ConfigError(f"unknown validator {validator!r} in rule {entry['id']}")
        path_pattern = entry.get("path")
        rules.append(
            Rule(
                rule_id=entry["id"],
                cls=entry["class"],
                regex=re.compile(entry["pattern"], flags),
                group=int(entry.get("group", 0)),
                validator=validator,
                path_regex=re.compile(path_pattern) if path_pattern else None,
            )
        )
    return rules, settings, validators


def load_local(path: Path | None) -> dict | None:
    raw = os.environ.get("JOBNIAC_PRIVACY_LOCAL_JSON")
    if raw:
        source = "environment variable JOBNIAC_PRIVACY_LOCAL_JSON"
        text = raw
    elif path is not None and path.is_file():
        source = path.name
        text = path.read_text(encoding="utf-8")
    else:
        return None
    try:
        data = json.loads(text)
    except ValueError as exc:
        raise ConfigError(f"{source} is not valid JSON") from exc
    literals = data.get("owner_literals", [])
    if not isinstance(literals, list) or not all(isinstance(x, str) and len(x) >= 2 for x in literals):
        raise ConfigError(f"{source}: owner_literals must be a list of strings of at least two characters")
    return data


def owner_rules(literals: list[str]) -> list[Rule]:
    rules = []
    for index, literal in enumerate(dict.fromkeys(literals), start=1):
        latin1 = literal.encode("utf-8").decode("latin-1")
        pattern = r"(?<![A-Za-z0-9])" + re.escape(latin1) + r"(?![A-Za-z0-9])"
        rules.append(Rule(f"owner-literal-{index}", "owner-identity", re.compile(pattern, re.IGNORECASE), 0, None, None))
    return rules


def load_phrases(path: Path | None) -> PhraseIndex:
    if path is None or not path.is_file():
        return EMPTY_PHRASES
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except ValueError as exc:
        raise ConfigError(f"{path.name} is not valid JSON") from exc
    if data.get("version") != 1 or data.get("normalisation") != NORMALISATION:
        raise ConfigError(f"{path.name} was made with another normalisation or version")
    digests = frozenset(data["digests"])
    return PhraseIndex(
        digests=digests,
        first_digests=frozenset(data["first_token_digests"]),
        lengths=tuple(sorted(set(int(n) for n in data["token_counts"]))),
        count=len(digests),
    )


# ---------------------------------------------------------------------------
# Content scanning
# ---------------------------------------------------------------------------


@dataclass
class Hit:
    cls: str
    rule_id: str
    line: int
    excerpt: str
    key: str


@dataclass
class ContentResult:
    sha256: str
    size: int
    hits: list[Hit]
    binary_kind: str | None


class LineIndex:
    """Maps a character offset to the number of newlines before it, in O(log n)."""

    def __init__(self, text: str) -> None:
        self._positions = [m.start() for m in re.finditer("\n", text)]

    def newlines_before(self, offset: int) -> int:
        return bisect.bisect_left(self._positions, offset)


def binary_kind(head: bytes, path: str | None) -> str | None:
    if head.startswith(b"%PDF-"):
        return "pdf"
    if head.startswith(b"\xff\xd8\xff"):
        return "jpeg"
    if head.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    if head.startswith((b"GIF87a", b"GIF89a")):
        return "gif"
    if head[:4] == b"RIFF" and head[8:12] == b"WEBP":
        return "webp"
    if head.startswith(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"):
        return "ole-document"
    if head.startswith(b"\x00\x00\x01\x00"):
        return "ico-image"
    if head.startswith(b"icns"):
        return "icns-image"
    if head.startswith(b"PK\x03\x04") and path and Path(path).suffix.lower() in OFFICE_EXTENSIONS:
        return "office-zip"
    return None


class ContentScanner:
    def __init__(self, config: ScanConfig, validators: dict[str, Callable[[str], bool]]) -> None:
        self._config = config
        self._validators = validators
        self._rules = config.rules + config.owner_rules

    def mask_text(self, text: str) -> str:
        """``text`` with every generic or owner match masked (used for names shown in output)."""
        latin1 = text.encode("utf-8", "surrogateescape").decode("latin-1")
        spans: list[tuple[int, int, str]] = []
        for rule in self._rules:
            for match in rule.regex.finditer(latin1):
                if rule.path_regex is not None:
                    continue
                start, end = match.span(rule.group)
                if start < 0:
                    continue
                value = match.group(rule.group)
                if rule.validator and not self._validators[rule.validator](value):
                    continue
                spans.append((start, end, value))
        if not spans:
            return ascii_safe(text)
        spans.sort()
        out, cursor = [], 0
        for start, end, value in spans:
            if start < cursor:
                continue
            out.append(latin1[cursor:start])
            out.append(mask(value))
            cursor = end
        out.append(latin1[cursor:])
        result = "".join(out).encode("latin-1").decode("utf-8", "replace")
        return ascii_safe(result)

    def scan(self, chunks: Iterable[bytes], primary_path: str | None) -> ContentResult:
        hasher = hashlib.sha256()
        size = 0
        head = b""
        carry = b""
        newlines_before_chunk = 0
        seen: set[str] = set()
        hits: list[Hit] = []
        # Owner literals and phrase fingerprints only apply to text: binary data is matched by magic bytes
        # and by the generic rules, which avoids short literals matching inside compressed or image data.
        text_mode: bool | None = None

        def add(hit_cls: str, rule_id: str, line: int, value_digest: str, excerpt: str) -> None:
            key = f"{hit_cls}|{rule_id}|{line}|{value_digest}"
            if key not in seen:
                seen.add(key)
                hits.append(Hit(hit_cls, rule_id, line, excerpt, key))

        for raw in chunks:
            if not raw:
                continue
            hasher.update(raw)
            size += len(raw)
            if len(head) < 16:
                head += raw[: 16 - len(head)]
            if text_mode is None:
                text_mode = binary_kind(raw[:16], primary_path) is None
            data = carry + raw
            latin1 = data.decode("latin-1")
            index = LineIndex(latin1)
            for rule in self._rules:
                if rule.cls == "owner-identity" and not text_mode:
                    continue
                if rule.path_regex is not None and (primary_path is None or not rule.path_regex.search(primary_path)):
                    continue
                for match in rule.regex.finditer(latin1):
                    value = match.group(rule.group)
                    if value is None:
                        continue
                    if rule.validator and not self._validators[rule.validator](value):
                        continue
                    line = newlines_before_chunk + index.newlines_before(match.start(rule.group)) + 1
                    digest = hashlib.sha256(value.encode("utf-8", "surrogateescape")).hexdigest()[:16]
                    add(rule.cls, rule.rule_id, line, digest, mask(value))
            if self._config.phrases.count and text_mode:
                self._phrase_hits(data.decode("utf-8", "replace"), newlines_before_chunk, add)
            if len(data) > OVERLAP_BYTES:
                committed = data[: len(data) - OVERLAP_BYTES]
                newlines_before_chunk += committed.count(b"\n")
                carry = data[len(data) - OVERLAP_BYTES :]
            else:
                carry = data

        kind = binary_kind(head, primary_path)
        if kind is not None:
            hits.append(
                Hit(
                    "binary-document",
                    f"magic-{kind}",
                    0,
                    f"<binary {kind} file, {size} bytes>",
                    f"binary|{kind}",
                )
            )
        return ContentResult(hasher.hexdigest(), size, hits, kind)

    def _phrase_hits(self, text: str, newlines_before_chunk: int, add: Callable[..., None]) -> None:
        normalised = unicodedata.normalize("NFC", text).casefold()
        index = LineIndex(normalised)
        matches = list(TOKEN_RE.finditer(normalised))
        tokens = [m.group() for m in matches]
        phrases = self._config.phrases
        for start, token in enumerate(tokens):
            # Cheap prefilter: only windows that start with a known first token are hashed in full.
            if hashlib.sha256(token.encode("utf-8")).hexdigest() not in phrases.first_digests:
                continue
            for length in phrases.lengths:
                if start + length > len(tokens):
                    continue
                digest = phrase_digest(tokens[start : start + length])
                if digest in phrases.digests:
                    line = newlines_before_chunk + index.newlines_before(matches[start].start()) + 1
                    add(
                        "personal-phrase",
                        "phrase-fingerprint",
                        line,
                        digest[:16],
                        f"<phrase fingerprint match, {length} tokens>",
                    )


# ---------------------------------------------------------------------------
# Repository access
# ---------------------------------------------------------------------------


def run_git(repo: Path, args: list[str], input_bytes: bytes | None = None) -> bytes:
    try:
        completed = subprocess.run(
            ["git", *args],
            cwd=repo,
            input=input_bytes,
            capture_output=True,
            check=False,
        )
    except OSError as exc:
        raise ConfigError(f"cannot run git: {exc}") from exc
    if completed.returncode != 0:
        raise ConfigError(f"git {args[0]} failed with exit status {completed.returncode}")
    return completed.stdout


def unquote_git_path(path: str) -> str:
    if len(path) >= 2 and path.startswith('"') and path.endswith('"'):
        inner = path[1:-1]
        raw = bytes(inner, "utf-8").decode("unicode_escape").encode("latin-1", "replace")
        return raw.decode("utf-8", "surrogateescape")
    return path


def iter_worktree_files(repo: Path) -> Iterator[tuple[str, Path]]:
    listing = run_git(repo, ["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
    for raw in listing.split(b"\0"):
        if not raw:
            continue
        relative = raw.decode("utf-8", "surrogateescape")
        full = repo / relative
        if full.is_file() and not full.is_symlink():
            yield relative, full


def stream_objects(repo: Path, shas: list[str]) -> Iterator[tuple[str, str, Iterator[bytes]]]:
    """Yield (sha, type, chunk iterator) for each object, reading ``git cat-file --batch`` in order."""
    try:
        proc = subprocess.Popen(
            ["git", "cat-file", "--batch"],
            cwd=repo,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
    except OSError as exc:
        raise ConfigError(f"cannot run git cat-file: {exc}") from exc
    assert proc.stdin is not None and proc.stdout is not None

    def feed() -> None:
        try:
            for sha in shas:
                proc.stdin.write(sha.encode("ascii") + b"\n")
            proc.stdin.close()
        except OSError:
            pass

    writer = threading.Thread(target=feed, daemon=True)
    writer.start()
    finished = False
    try:
        for sha in shas:
            header = proc.stdout.readline().split()
            if len(header) != 3 or header[0].decode("ascii") != sha:
                raise ConfigError("git cat-file returned an unexpected header")
            otype = header[1].decode("ascii")
            remaining = int(header[2])

            def chunks(count: int = remaining) -> Iterator[bytes]:
                left = count
                while left > 0:
                    part = proc.stdout.read(min(CHUNK_BYTES, left))
                    if not part:
                        raise ConfigError("git cat-file ended early")
                    left -= len(part)
                    yield part

            body = chunks()
            yield sha, otype, body
            for _ in body:  # drain whatever the consumer did not read
                pass
            proc.stdout.read(1)  # trailing newline after each object
        finished = True
    finally:
        if not finished:
            proc.kill()
        writer.join(timeout=5)
        proc.stdout.close()
        returncode = proc.wait()
    if finished and returncode != 0:
        raise ConfigError(f"git cat-file failed with exit status {returncode}")


def strip_structural_lines(content: bytes) -> bytes:
    """Drop tree, parent and object id lines of commit and tag objects (hex ids only, no content)."""
    kept = [
        line
        for line in content.split(b"\n")
        if not line.startswith((b"tree ", b"parent ", b"object "))
    ]
    return b"\n".join(kept)


@dataclass
class Occurrence:
    source: str  # worktree | history | metadata | name | ref
    where: str  # masked display location
    cls: str
    rule_id: str
    line: int
    excerpt: str
    content_sha256: str | None
    blob: str | None
    paths: list[str]
    commits: list[str]


def history_plan(repo: Path) -> tuple[list[str], dict[str, set[str]], dict[str, set[str]], dict[str, set[str]], list[str], dict[str, str]]:
    """Object ids, blob to commits, blob to paths, path to commits, all path names, ref names -> object id."""
    refs_raw = run_git(repo, ["for-each-ref", "--format=%(objectname) %(objecttype) %(refname)"])
    refs: dict[str, str] = {}
    ref_tips: list[str] = []
    for line in refs_raw.decode("utf-8", "surrogateescape").splitlines():
        if not line.strip():
            continue
        sha, _otype, name = line.split(" ", 2)
        refs[name] = sha
        ref_tips.append(sha)

    listing = run_git(repo, ["rev-list", "--all", "--objects"]).decode("utf-8", "surrogateescape").splitlines()
    shas: list[str] = []
    seen_shas: set[str] = set()
    first_path: dict[str, str] = {}
    for line in listing:
        sha, _, path = line.partition(" ")
        if sha not in seen_shas:
            seen_shas.add(sha)
            shas.append(sha)
        if path and sha not in first_path:
            first_path[sha] = path
    for tip in ref_tips:
        if tip not in seen_shas:
            seen_shas.add(tip)
            shas.append(tip)

    log = run_git(
        repo,
        ["-c", "core.quotePath=false", "log", "--all", "--root", "-m", "--raw", "--no-renames", "--no-abbrev", "--format=@%H"],
    ).decode("utf-8", "surrogateescape").splitlines()
    blob_commits: dict[str, set[str]] = {}
    blob_paths: dict[str, set[str]] = {}
    path_commits: dict[str, set[str]] = {}
    commit = ""
    zero = "0" * 40
    for line in log:
        if line.startswith("@"):
            commit = line[1:]
            continue
        if not line.startswith(":"):
            continue
        meta, _, raw_path = line.partition("\t")
        fields = meta.split()
        if len(fields) < 5:
            continue
        new_sha, status = fields[3], fields[4]
        path = unquote_git_path(raw_path)
        path_commits.setdefault(path, set()).add(commit)
        if status.startswith("D") or new_sha == zero:
            continue
        blob_commits.setdefault(new_sha, set()).add(commit)
        blob_paths.setdefault(new_sha, set()).add(path)
    for sha, path in first_path.items():
        blob_paths.setdefault(sha, set()).add(path)
        path_commits.setdefault(path, set())
    names = sorted(path_commits)
    return shas, blob_commits, blob_paths, path_commits, names, refs


def load_allowlist(path: Path) -> dict[str, dict[str, str]]:
    """Map content SHA-256 to {class: reason}. Every entry needs a reason and a 64-hex digest."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ConfigError(f"cannot read allow-list {path.name}: {exc}") from exc
    if data.get("version") != 1:
        raise ConfigError("unsupported allow-list version")
    table: dict[str, dict[str, str]] = {}
    for entry in data.get("entries", []):
        digest = entry.get("sha256", "")
        if not re.fullmatch(r"[0-9a-f]{64}", digest):
            raise ConfigError("allow-list entry without a valid sha256")
        if not str(entry.get("reason", "")).strip():
            raise ConfigError(f"allow-list entry {digest[:12]} has no reason")
        classes = entry.get("classes", [])
        if not classes:
            raise ConfigError(f"allow-list entry {digest[:12]} lists no classes")
        for cls in classes:
            table.setdefault(digest, {})[cls] = entry["reason"]
    return table


# ---------------------------------------------------------------------------
# Scan driver
# ---------------------------------------------------------------------------


@dataclass
class Report:
    occurrences: list[Occurrence] = field(default_factory=list)
    suppressed: dict[str, int] = field(default_factory=dict)
    used_allow: set[tuple[str, str]] = field(default_factory=set)
    scanned_objects: int = 0
    scanned_bytes: int = 0


def record(
    report: Report,
    allow: dict[str, dict[str, str]],
    occurrence: Occurrence,
) -> None:
    if occurrence.content_sha256 is not None:
        reasons = allow.get(occurrence.content_sha256, {})
        if occurrence.cls in reasons:
            report.suppressed[occurrence.cls] = report.suppressed.get(occurrence.cls, 0) + 1
            report.used_allow.add((occurrence.content_sha256, occurrence.cls))
            return
    report.occurrences.append(occurrence)


def scan_names(scanner: ContentScanner, names: Iterable[str]) -> Iterator[tuple[str, Hit]]:
    for name in names:
        result = scanner.scan([name.encode("utf-8", "surrogateescape")], None)
        for hit in result.hits:
            yield name, hit


def run_scan(
    repo: Path,
    mode: str,
    config: ScanConfig,
    validators: dict[str, Callable[[str], bool]],
    allow: dict[str, dict[str, str]],
    progress: bool,
) -> Report:
    scanner = ContentScanner(config, validators)
    report = Report()

    if mode in ("all", "worktree"):
        for relative, full in iter_worktree_files(repo):
            with full.open("rb") as handle:
                chunks = iter(lambda: handle.read(CHUNK_BYTES), b"")
                result = scanner.scan(chunks, relative)
            report.scanned_objects += 1
            report.scanned_bytes += result.size
            shown_path = scanner.mask_text(relative)
            for hit in result.hits:
                record(
                    report,
                    allow,
                    Occurrence("worktree", shown_path, hit.cls, hit.rule_id, hit.line, hit.excerpt,
                               result.sha256, None, [relative], []),
                )
            for hit in scan_names(scanner, [relative]):
                record(
                    report,
                    allow,
                    Occurrence("name", scanner.mask_text(hit[0]), hit[1].cls, hit[1].rule_id, 0,
                               hit[1].excerpt, None, None, [relative], []),
                )
        if progress:
            print(f"working tree: {report.scanned_objects} files, {report.scanned_bytes} bytes", file=sys.stderr)

    if mode in ("all", "history"):
        shas, blob_commits, blob_paths, path_commits, names, refs = history_plan(repo)
        for ref_name, tip in refs.items():
            for hit in scan_names(scanner, [ref_name]):
                record(
                    report,
                    allow,
                    Occurrence("ref", scanner.mask_text(ref_name), hit[1].cls, hit[1].rule_id, 0,
                               hit[1].excerpt, None, None, [], [tip]),
                )
        for name in names:
            commits = sorted(path_commits.get(name, set()))
            for hit in scan_names(scanner, [name]):
                record(
                    report,
                    allow,
                    Occurrence("name", scanner.mask_text(name), hit[1].cls, hit[1].rule_id, 0,
                               hit[1].excerpt, None, None, [name], commits),
                )

        counted = 0
        for sha, otype, body in stream_objects(repo, shas):
            counted += 1
            if otype == "blob":
                paths = sorted(blob_paths.get(sha, set()))
                commits = sorted(blob_commits.get(sha, set()))
                result = scanner.scan(body, paths[0] if paths else None)
                report.scanned_objects += 1
                report.scanned_bytes += result.size
                display = scanner.mask_text(paths[0]) if paths else "<unattributed>"
                for hit in result.hits:
                    record(
                        report,
                        allow,
                        Occurrence("history", f"blob {sha[:7]} {display}", hit.cls, hit.rule_id,
                                   hit.line, hit.excerpt, result.sha256, sha, paths, commits),
                    )
            elif otype in ("commit", "tag"):
                content = strip_structural_lines(b"".join(body))
                result = scanner.scan([content], None)
                report.scanned_objects += 1
                report.scanned_bytes += result.size
                for hit in result.hits:
                    record(
                        report,
                        allow,
                        Occurrence("metadata", f"{otype} {sha[:7]}", hit.cls, hit.rule_id, hit.line,
                                   hit.excerpt, None, None, [], [sha]),
                    )
            else:
                for _ in body:
                    pass
        if progress:
            print(f"history: {counted} objects read", file=sys.stderr)
    return report


def print_report(report: Report, config: ScanConfig, allow: dict[str, dict[str, str]], limit: int) -> None:
    by_class: dict[str, list[Occurrence]] = {}
    for occurrence in report.occurrences:
        by_class.setdefault(occurrence.cls, []).append(occurrence)
    print("Privacy scan summary (occurrences / distinct content objects / suppressed by allow-list):")
    classes = sorted(set(by_class) | set(report.suppressed))
    if not classes:
        print("  no findings")
    for cls in classes:
        items = by_class.get(cls, [])
        distinct = {(o.content_sha256 or o.where) for o in items}
        print(f"  {cls}: {len(items)} / {len(distinct)} / {report.suppressed.get(cls, 0)}")
    print(f"Scanned: {report.scanned_objects} content objects, {report.scanned_bytes} bytes.")
    print(
        f"Owner literals: {config.owner_literal_count}; phrase fingerprints: {config.phrases.count}."
        + ("" if config.owner_literal_count else " Owner-specific classes were NOT checked (no local file).")
    )
    unused = [digest for digest in allow if not any((digest, cls) in report.used_allow for cls in allow[digest])]
    if unused:
        print(f"Allow-list entries not seen in this scan: {len(unused)} (review and remove if obsolete).")
    if report.occurrences:
        print("Findings (masked):")
        for occurrence in report.occurrences[:limit]:
            location = f"{occurrence.source} {occurrence.where}"
            line = f" line {occurrence.line}" if occurrence.line else ""
            print(f"  [{occurrence.cls}] {location}{line} rule={occurrence.rule_id} value={ascii_safe(occurrence.excerpt)}")
        if len(report.occurrences) > limit:
            print(f"  ... {len(report.occurrences) - limit} more (see --report)")


def write_report(path: Path, report: Report) -> None:
    rows = []
    for occurrence in report.occurrences:
        rows.append(
            {
                "class": occurrence.cls,
                "rule": occurrence.rule_id,
                "source": occurrence.source,
                "line": occurrence.line,
                "excerpt": ascii_safe(occurrence.excerpt),
                "content_sha256": occurrence.content_sha256,
                "blob": occurrence.blob,
                "paths": occurrence.paths,
                "commits": occurrence.commits,
            }
        )
    payload = {"findings": rows, "suppressed": report.suppressed}
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=True) + "\n", encoding="utf-8")


def build_config(args: argparse.Namespace) -> tuple[ScanConfig, dict[str, Callable[[str], bool]], dict | None]:
    rules, settings, validators = load_generic_patterns(Path(args.patterns))
    local = load_local(Path(args.local) if args.local else None)
    if args.require_local and local is None:
        raise ConfigError("owner-specific local file is required by --require-local but is absent")
    literals = list(local.get("owner_literals", [])) if local else []
    phrases = load_phrases(Path(args.phrases) if args.phrases else None)
    if args.require_local and not phrases.count:
        raise ConfigError("phrase fingerprint file is required by --require-local but is absent")
    config = ScanConfig(
        rules=rules,
        settings=settings,
        owner_rules=owner_rules(literals),
        phrases=phrases,
        owner_literal_count=len(set(literals)),
    )
    return config, validators, local


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Privacy pattern scan of the working tree and git history.")
    parser.add_argument("--repo", default=str(REPO_ROOT), help="repository root (default: this checkout)")
    parser.add_argument("--mode", choices=("all", "worktree", "history"), default="all")
    parser.add_argument("--patterns", default=str(DEFAULT_PATTERNS))
    parser.add_argument("--allowlist", default=str(DEFAULT_ALLOWLIST))
    parser.add_argument("--local", default=str(DEFAULT_LOCAL), help="gitignored owner-specific JSON file")
    parser.add_argument("--phrases", default=str(DEFAULT_PHRASES), help="gitignored phrase fingerprint JSON")
    parser.add_argument("--require-local", action="store_true", help="fail when owner-specific inputs are absent")
    parser.add_argument("--report", help="write the full findings (exact paths and commits) to this JSON file")
    parser.add_argument("--limit", type=int, default=300, help="findings printed to stdout")
    args = parser.parse_args(argv)

    try:
        repo = Path(args.repo).resolve()
        config, validators, _local = build_config(args)
        allow = load_allowlist(Path(args.allowlist))
        report = run_scan(repo, args.mode, config, validators, allow, progress=True)
    except ConfigError as exc:
        print(f"privacy scan error: {ascii_safe(str(exc))}", file=sys.stderr)
        return 2

    print_report(report, config, allow, args.limit)
    if args.report:
        write_report(Path(args.report), report)
    return 1 if report.occurrences else 0


if __name__ == "__main__":
    sys.exit(main())
