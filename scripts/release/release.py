"""Release helpers for the JobniacFast release workflow (stdlib only).

Run locally with `py -I scripts/release/release.py <command>`; in CI with `python` (Windows runners)
or `python3` (Linux runners) after actions/setup-python. All release logic lives in this one
module, with unit tests in scripts/release/tests.

Every command fails closed: it prints one short `FAIL:` line and exits with status 1. GitHub API
calls never print response bodies or tokens.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import urllib.error
import uuid
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

SCHEMA_INVENTORY = "jobniac-payload-inventory/1"
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(-dev\.[0-9]+)?$")
MERGE_SHA_RE = re.compile(r"^[0-9a-f]{40}$")
TAG_PEEL_LIMIT = 8
API_TIMEOUT_SECONDS = 30
API_MAX_BYTES = 1024 * 1024
UPLOAD_TIMEOUT_SECONDS = 600
API_VERSION_HEADER = "2022-11-28"
PLATFORMS = ("windows-x64", "linux-x64")
PRODUCT = "JobniacFast"
LEG_DIGESTS = "leg-digests.txt"
SHA256SUMS = "SHA256SUMS"
# The updater stays off: these names would mean that it was turned on.
FORBIDDEN_UPDATER_NAMES = ("latest.json",)
FORBIDDEN_UPDATER_SUFFIXES = (".sig",)
# Only the installed tree may hold these names beyond the staged payload.
INSTALLED_ONLY_ALLOWLIST = ("uninstall.exe",)
PRUNE_PAGE_SIZE = 20
PRUNE_MAX_PAGES = 50

REPO_ROOT = Path(__file__).resolve().parents[2]


class ReleaseError(Exception):
    """A gate failed. The message is printed as is and never contains secrets or response bodies."""


# ---------------------------------------------------------------------------
# Version intent and tags
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ReleaseIntent:
    version: str
    tag: str
    prerelease: bool


def tag_for_version(version: str) -> ReleaseIntent:
    if not VERSION_RE.fullmatch(version):
        raise ReleaseError("version rejected: only X.Y.Z and X.Y.Z-dev.N are drafted; publication stays manual")
    return ReleaseIntent(version=version, tag=f"v{version}", prerelease="-dev." in version)


def read_versions(repo_root: Path) -> dict[str, str]:
    cargo = tomllib.loads((repo_root / "Cargo.toml").read_text(encoding="utf-8"))
    tauri = json.loads((repo_root / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
    package = json.loads((repo_root / "package.json").read_text(encoding="utf-8"))
    return {
        "Cargo.toml [workspace.package]": cargo["workspace"]["package"]["version"],
        "src-tauri/tauri.conf.json": tauri["version"],
        "package.json": package["version"],
    }


def resolve_version(repo_root: Path) -> ReleaseIntent:
    versions = read_versions(repo_root)
    if len(set(versions.values())) != 1:
        detail = ", ".join(f"{name}={value}" for name, value in versions.items())
        raise ReleaseError(f"version fields differ: {detail}")
    return tag_for_version(next(iter(versions.values())))


def check_merge_sha(value: str) -> str:
    if not MERGE_SHA_RE.fullmatch(value):
        raise ReleaseError("MERGE_SHA is not a 40-character commit id")
    return value


def check_checkout(repo_root: Path, merge_sha: str) -> None:
    check_merge_sha(merge_sha)
    completed = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo_root, capture_output=True, encoding="utf-8", errors="replace", check=False
    )
    if completed.returncode != 0 or completed.stdout.strip() != merge_sha:
        raise ReleaseError("checkout HEAD is not MERGE_SHA")


# ---------------------------------------------------------------------------
# Hashing, digest manifests and the exact asset set
# ---------------------------------------------------------------------------


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def list_files(directory: Path, exclude: set[str]) -> list[str]:
    return sorted(entry.name for entry in directory.iterdir() if entry.is_file() and entry.name not in exclude)


def write_digest_manifest(directory: Path, out_name: str, exclude: set[str]) -> Path:
    """Writes `<hex>  <name>` lines (sha256sum format, sorted, LF) for the files in `directory`."""
    out = directory / out_name
    names = list_files(directory, exclude | {out_name})
    text = "".join(f"{sha256_file(directory / name)}  {name}\n" for name in names)
    out.write_bytes(text.encode("utf-8"))
    return out


def verify_digest_manifest(directory: Path, out_name: str, exclude: set[str]) -> int:
    """Re-reads the manifest and compares every line with a fresh digest of the file it names."""
    manifest = directory / out_name
    raw = manifest.read_bytes()
    if b"\r" in raw or not raw.endswith(b"\n"):
        raise ReleaseError(f"{out_name} must use LF line endings and end with a newline")
    lines = raw.decode("utf-8").split("\n")[:-1]
    entries: list[tuple[str, str]] = []
    for line in lines:
        match = re.fullmatch(r"([0-9a-f]{64})  ([^/\\\n]+)", line)
        if not match:
            raise ReleaseError(f"{out_name} has a malformed line")
        entries.append((match.group(2), match.group(1)))
    names = [name for name, _ in entries]
    if names != sorted(names):
        raise ReleaseError(f"{out_name} names are not sorted")
    if names != list_files(directory, exclude | {out_name}):
        raise ReleaseError(f"{out_name} does not list exactly the files beside it")
    for name, digest in entries:
        if sha256_file(directory / name) != digest:
            raise ReleaseError(f"{out_name} digest mismatch for {name}")
    return len(entries)


def write_sha256sums(directory: Path) -> Path:
    return write_digest_manifest(directory, SHA256SUMS, set())


def verify_sha256sums(directory: Path) -> int:
    return verify_digest_manifest(directory, SHA256SUMS, set())


def expected_asset_names(version: str) -> list[str]:
    """The release asset set with the Tauri bundle names."""
    names = [
        f"{PRODUCT}_{version}_x64-setup.exe",
        f"{PRODUCT}_{version}_amd64.AppImage",
        f"{PRODUCT}_{version}_amd64.deb",
        f"{PRODUCT}-{version}-desktop.cargo.cdx.json",
        f"{PRODUCT}-{version}-cli.cargo.cdx.json",
        f"{PRODUCT}-{version}.npm.cdx.json",
        "THIRD-PARTY-NOTICES.txt",
        f"{PRODUCT}-{version}-windows-x64.payload-inventory.json",
        f"{PRODUCT}-{version}-linux-x64.payload-inventory.json",
        SHA256SUMS,
    ]
    return sorted(names)


def check_exact_asset_set(directory: Path, version: str) -> list[str]:
    present = sorted(entry.name for entry in directory.iterdir())
    for name in present:
        lowered = name.lower()
        if lowered in FORBIDDEN_UPDATER_NAMES or lowered.endswith(FORBIDDEN_UPDATER_SUFFIXES):
            raise ReleaseError("updater artifact present: the updater stays off")
    expected = expected_asset_names(version)
    missing = sorted(set(expected) - set(present))
    extra = sorted(set(present) - set(expected))
    if missing or extra:
        raise ReleaseError(f"asset set mismatch: {len(missing)} missing, {len(extra)} unexpected")
    if any(not (directory / name).is_file() for name in present):
        raise ReleaseError("asset set contains a non-file entry")
    return present


# ---------------------------------------------------------------------------
# Payload inventory and notices reconciliation
# ---------------------------------------------------------------------------


def load_json(path: Path) -> dict[str, object]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ReleaseError(f"{path.name} is not a JSON object")
    return data


def write_json(path: Path, data: object) -> None:
    text = json.dumps(data, indent=2, sort_keys=True, ensure_ascii=True) + "\n"
    path.write_bytes(text.encode("utf-8"))


def build_inventory(payload: Path, owners_path: Path, components_path: Path, notices_manifest: Path) -> dict[str, object]:
    """Hashes every payload file, attributes it to an owner and reconciles it with the notices."""
    rules = load_json(owners_path)["rules"]
    components = load_json(components_path)["components"]
    notice_ids = {entry["id"] for entry in load_json(notices_manifest)["components"]}  # type: ignore[index,union-attr]
    files = sorted(
        (candidate for candidate in payload.rglob("*") if candidate.is_file()),
        key=lambda candidate: candidate.relative_to(payload).as_posix(),
    )
    entries: list[dict[str, object]] = []
    for path in files:
        relative = path.relative_to(payload).as_posix()
        owner = next((rule["owner"] for rule in rules if rule["path"] == relative), None)  # type: ignore[index,call-overload]
        if owner is None:
            raise ReleaseError("payload file without an owner")
        entries.append({"path": relative, "size": path.stat().st_size, "sha256": sha256_file(path), "owner": owner})
    present = {str(entry["path"]) for entry in entries}
    for component in components:  # type: ignore[attr-defined]
        if component["id"] not in notice_ids:
            raise ReleaseError(f"static component without a notice entry: {component['id']}")
        if component.get("external"):
            continue
        if not any(candidate in present for candidate in component["payload_any"]):
            raise ReleaseError(f"notice entry without a payload file: {component['id']}")
    for entry in entries:
        owner = str(entry["owner"])
        if owner.startswith("static:") and owner.split(":", 1)[1] not in notice_ids:
            raise ReleaseError("payload owner without a notice entry")
    return {"schema": SCHEMA_INVENTORY, "files": entries}


def compare_installed(inventory_path: Path, installed: Path, allowlist: tuple[str, ...]) -> int:
    """Compares the staged inventory with an installed tree by path, size and digest."""
    staged = {str(entry["path"]): entry for entry in load_json(inventory_path)["files"]}  # type: ignore[index,union-attr]
    actual = {
        path.relative_to(installed).as_posix(): path
        for path in installed.rglob("*")
        if path.is_file()
    }
    for name in sorted(set(actual) - set(staged)):
        if name not in allowlist:
            raise ReleaseError("installed tree has a file that the staged payload does not")
    for name, entry in staged.items():
        if name not in actual:
            raise ReleaseError("installed tree lacks a staged payload file")
        path = actual[name]
        if path.stat().st_size != entry["size"] or sha256_file(path) != entry["sha256"]:
            raise ReleaseError("installed file differs from the staged payload")
    return len(staged)


SBOM_NAMESPACE = uuid.UUID("6f1c2a52-7d1e-4f0b-9a57-3c2d8e4b1a90")


def normalize_sbom(source: Path, destination: Path, seed: str) -> None:
    """Makes a CycloneDX SBOM byte-reproducible.

    cargo-cyclonedx 0.5.9 and `pnpm sbom` write a random serial number, a creation timestamp and
    components in an unstable order (see the test in scripts/release/tests). The
    release drops the timestamp, derives the serial number from `seed` (tag and MERGE_SHA plus the
    file role), sorts the top-level components and dependencies, and writes LF JSON.
    """
    data = json.loads(source.read_text(encoding="utf-8"))
    if not isinstance(data, dict) or data.get("bomFormat") != "CycloneDX":
        raise ReleaseError("input is not a CycloneDX JSON document")
    data["serialNumber"] = f"urn:uuid:{uuid.uuid5(SBOM_NAMESPACE, seed)}"
    metadata = data.get("metadata")
    if isinstance(metadata, dict):
        metadata.pop("timestamp", None)
    for key in ("components", "dependencies"):
        if isinstance(data.get(key), list):
            data[key] = sorted(data[key], key=lambda item: json.dumps(item, sort_keys=True))
    text = json.dumps(data, indent=2, ensure_ascii=False) + "\n"
    destination.write_bytes(text.encode("utf-8"))


def stage_sidecar(target_dir: Path, binaries_dir: Path, triple: str) -> Path:
    """Copies the built jobniac-cli into the externalBin location Tauri expects."""
    suffix = ".exe" if "windows" in triple else ""
    source = target_dir / f"jobniac-cli{suffix}"
    if not source.is_file():
        raise ReleaseError("jobniac-cli is not built")
    binaries_dir.mkdir(parents=True, exist_ok=True)
    destination = binaries_dir / f"jobniac-cli-{triple}{suffix}"
    shutil.copyfile(source, destination)
    if sha256_file(destination) != sha256_file(source):
        raise ReleaseError("sidecar copy differs from the built binary")
    return destination


def host_triple() -> str:
    completed = subprocess.run(["rustc", "--print", "host-tuple"], capture_output=True, encoding="utf-8", errors="replace", check=False)
    triple = completed.stdout.strip()
    if completed.returncode != 0 or not triple:
        raise ReleaseError("rustc --print host-tuple failed (Rust 1.84 or later is required)")
    return triple


def static_text(source: dict[str, str], sysroot: Path | None, pdfium_dirs: list[Path], allow_missing: bool) -> list[tuple[str, bytes]]:
    """Returns (label, verbatim bytes) pairs for one text source of a static component."""
    kind = source["kind"]
    if kind == "pdfium-archive":
        out: list[tuple[str, bytes]] = []
        seen: set[str] = set()
        for directory in pdfium_dirs:
            candidates = [directory / "LICENSE", *sorted((directory / "licenses").glob("*"))]
            for candidate in candidates:
                if not candidate.is_file():
                    raise ReleaseError("a PDFium licence file is missing")
                digest = sha256_file(candidate)
                if digest not in seen:
                    seen.add(digest)
                    out.append((f"PDFium {candidate.name}", candidate.read_bytes()))
        if not out:
            raise ReleaseError("no PDFium licence files were found")
        return out
    if kind == "repo":
        path = REPO_ROOT / source["path"]
    elif kind == "static":
        path = REPO_ROOT / "scripts" / "release" / "notices" / "texts" / source["path"]
    elif kind == "sysroot":
        if sysroot is None:
            raise ReleaseError("the Rust sysroot is required for the Rust notices")
        path = sysroot / source["path"]
    else:
        raise ReleaseError("unknown licence text source")
    if not path.is_file():
        if allow_missing:
            return [(source["label"], b"[TEXT NOT SUPPLIED: DRAFT ONLY]")]
        raise ReleaseError(f"licence text missing for a static component: {source['label']}")
    return [(source["label"], path.read_bytes())]


def npm_license_texts(packages: list[dict[str, object]]) -> tuple[list[str], list[str]]:
    """Returns (sections, failures) for the production npm packages. Local paths are never printed."""
    sections: list[str] = []
    failures: list[str] = []
    name_re = re.compile(r"^(LICEN[SC]E|COPYING|NOTICE)([.-].*)?$", re.IGNORECASE)
    for package in packages:
        name, version, directory = str(package["name"]), str(package["version"]), Path(str(package["path"]))
        files = sorted(entry for entry in directory.iterdir() if entry.is_file() and name_re.match(entry.name)) if directory.is_dir() else []
        if not files:
            # A package that ships no licence file gets a verbatim upstream copy, recorded beside it.
            override = REPO_ROOT / "scripts" / "release" / "notices" / "npm-overrides" / f"{name}@{version}"
            files = sorted(entry for entry in override.iterdir() if entry.is_file() and name_re.match(entry.name)) if override.is_dir() else []
        if not files:
            failures.append(f"{name}@{version}")
            continue
        block = [f"== npm: {name}@{version} (licence: {package['license']}) =="]
        for entry in files:
            block.append(f"--- {entry.name} ---")
            block.append(entry.read_bytes().decode("utf-8", errors="replace").rstrip("\n"))
        sections.append("\n".join(block))
    return sections, failures


def collect_npm_packages(npm_json: dict[str, list[dict[str, object]]]) -> list[dict[str, object]]:
    packages: list[dict[str, object]] = []
    for license_name, entries in npm_json.items():
        for entry in entries:
            for version, path in zip(entry["versions"], entry["paths"]):  # type: ignore[arg-type]
                packages.append({"name": entry["name"], "version": version, "license": entry.get("license", license_name), "path": path})
    packages.sort(key=lambda item: (str(item["name"]), str(item["version"])))
    return packages


def build_notices(
    out_dir: Path,
    pdfium_dirs: list[Path],
    cargo_about_text: str,
    npm_json: dict[str, list[dict[str, object]]],
    components_path: Path,
    sysroot: Path | None,
    allow_missing: bool,
) -> dict[str, object]:
    components = load_json(components_path)["components"]
    project_license = (REPO_ROOT / "LICENSE").read_bytes().decode("utf-8")
    sections = [
        "== JobniacFast (MIT) ==\n" + project_license.rstrip("\n"),
        "== Rust crates (cargo-about, licences accepted by about.toml) ==\n" + cargo_about_text.rstrip("\n"),
    ]
    packages = collect_npm_packages(npm_json)
    npm_sections, npm_failures = npm_license_texts(packages)
    if npm_failures:
        raise ReleaseError("npm production packages without a licence file: " + ", ".join(npm_failures))
    sections.append("== npm production dependencies (pnpm licenses list; texts from each package) ==\n" + "\n\n".join(npm_sections))
    for component in components:  # type: ignore[attr-defined]
        blocks = [f"== {component['title']} =="]
        for source in component["texts"]:
            for label, data in static_text(source, sysroot, pdfium_dirs, allow_missing):
                blocks.append(f"--- {label} ---")
                blocks.append(data.decode("utf-8", errors="replace").rstrip("\n"))
        sections.append("\n".join(blocks))
    text = ("\n\n".join(sections) + "\n").replace("\r\n", "\n")
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "THIRD-PARTY-NOTICES.txt").write_bytes(text.encode("utf-8"))
    manifest = {
        "schema": "jobniac-notices-manifest/1",
        "npm_packages": len(packages),
        "components": [{"id": component["id"]} for component in components],  # type: ignore[index]
        "text_sha256": hashlib.sha256(text.encode("utf-8")).hexdigest(),
    }
    write_json(out_dir / "notices-manifest.json", manifest)
    return manifest


def run_cargo_about() -> str:
    """cargo-about writes to a file with -o (its stdout redirection is refused on PowerShell)."""
    with tempfile.TemporaryDirectory() as scratch:
        output = Path(scratch) / "about.txt"
        completed = subprocess.run(
            ["cargo", "about", "generate", "--config", "about.toml", "--fail", "--locked",
             "-o", str(output), "scripts/release/notices/about.hbs"],
            cwd=REPO_ROOT, capture_output=True, encoding="utf-8", errors="replace", check=False,
        )
        if completed.returncode != 0 or not output.is_file():
            raise ReleaseError("cargo about failed: a crate licence is not accepted or has no text")
        return output.read_text(encoding="utf-8")


def run_pnpm_licences() -> dict[str, list[dict[str, object]]]:
    completed = subprocess.run(
        ["pnpm", "licenses", "list", "--prod", "--json"], cwd=REPO_ROOT, capture_output=True, encoding="utf-8", errors="replace", check=False,
    )
    if completed.returncode != 0:
        raise ReleaseError("pnpm licenses list failed")
    return json.loads(completed.stdout)


def sysroot_path() -> Path:
    completed = subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, encoding="utf-8", errors="replace", check=False)
    if completed.returncode != 0:
        raise ReleaseError("rustc --print sysroot failed")
    return Path(completed.stdout.strip())


# ---------------------------------------------------------------------------
# Assembly of the release asset set from the platform legs
# ---------------------------------------------------------------------------


def assemble(legs_dir: Path, out: Path, version: str) -> list[str]:
    """Collects the leg artifacts, re-checks each leg's digests and copies one copy of every asset."""
    out.mkdir(parents=True, exist_ok=True)
    seen: dict[str, Path] = {}
    for leg in sorted(entry for entry in legs_dir.iterdir() if entry.is_dir()):
        verify_digest_manifest(leg, LEG_DIGESTS, set())
        for path in sorted(leg.iterdir()):
            if path.name == LEG_DIGESTS:
                continue
            if path.name in seen:
                # Shared files (notices, SBOMs) are built once; the legs must agree byte for byte.
                if sha256_file(path) != sha256_file(seen[path.name]):
                    raise ReleaseError(f"legs disagree on {path.name}")
                continue
            seen[path.name] = path
            shutil.copyfile(path, out / path.name)
    write_sha256sums(out)
    verify_sha256sums(out)
    check_exact_asset_set(out, version)
    return sorted(seen)


# ---------------------------------------------------------------------------
# GitHub API (draft creation, pruning, attachment, verification)
# ---------------------------------------------------------------------------


def _reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ReleaseError("duplicate JSON key in an API response")
        result[key] = value
    return result


Transport = Callable[[str, str, str, bytes | None, str], tuple[int, bytes]]


def default_transport(method: str, url: str, token: str, body: bytes | None, content_type: str) -> tuple[int, bytes]:
    request = urllib.request.Request(url, data=body, method=method)
    request.add_header("Accept", "application/vnd.github+json")
    request.add_header("X-GitHub-Api-Version", API_VERSION_HEADER)
    request.add_header("Authorization", f"Bearer {token}")
    if body is not None:
        request.add_header("Content-Type", content_type)
    timeout = UPLOAD_TIMEOUT_SECONDS if "uploads." in url else API_TIMEOUT_SECONDS
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:  # noqa: S310 - fixed GitHub host
            data = response.read(API_MAX_BYTES + 1)
            status = response.status
    except urllib.error.HTTPError as error:
        data = error.read(API_MAX_BYTES + 1)
        status = error.code
    except (urllib.error.URLError, TimeoutError) as error:
        raise ReleaseError("GitHub API request failed") from error
    if len(data) > API_MAX_BYTES:
        raise ReleaseError("GitHub API response is too large")
    return status, data


class GitHubApi:
    def __init__(self, repo: str, token: str, transport: Transport = default_transport) -> None:
        self.repo = repo
        self.token = token
        self.transport = transport

    def raw(self, method: str, path: str, payload: object | None = None, body: bytes | None = None,
            content_type: str = "application/json", base: str | None = None) -> tuple[int, bytes]:
        if body is None and payload is not None:
            body = json.dumps(payload).encode("utf-8")
        url = f"{base or 'https://api.github.com/repos/' + self.repo}{path}"
        return self.transport(method, url, self.token, body, content_type)

    def call(self, method: str, path: str, payload: object | None = None, ok: tuple[int, ...] = (200,)) -> object:
        status, data = self.raw(method, path, payload)
        if status not in ok:
            raise ReleaseError(f"GitHub API {method} failed")
        if not data:
            return None
        try:
            return json.loads(data.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ReleaseError("GitHub API response is not JSON") from error

    def releases_for_tag(self, tag: str) -> list[dict[str, object]]:
        found: list[dict[str, object]] = []
        for page in range(1, PRUNE_MAX_PAGES + 1):
            batch = self.call("GET", f"/releases?per_page={PRUNE_PAGE_SIZE}&page={page}")
            if not isinstance(batch, list):
                raise ReleaseError("releases listing is not a list")
            found.extend(item for item in batch if item.get("tag_name") == tag)  # type: ignore[union-attr]
            if len(batch) < PRUNE_PAGE_SIZE:
                return found
        raise ReleaseError("releases listing exceeds the page cap")

    def tag_commit(self, tag: str) -> str | None:
        """The commit a tag peels to, or None when the tag does not exist (peel limit 8)."""
        status, data = self.raw("GET", f"/git/ref/tags/{tag}")
        if status == 404:
            return None
        if status != 200:
            raise ReleaseError("GitHub API GET failed")
        obj = json.loads(data.decode("utf-8"), object_pairs_hook=_reject_duplicate_keys)["object"]
        sha, kind = obj["sha"], obj["type"]
        for _ in range(TAG_PEEL_LIMIT + 1):
            if kind == "commit":
                return str(sha)
            if kind != "tag":
                raise ReleaseError("tag does not peel to a commit")
            obj = self.call("GET", f"/git/tags/{sha}")["object"]  # type: ignore[index]
            sha, kind = obj["sha"], obj["type"]  # type: ignore[index]
        raise ReleaseError("tag peel limit exceeded")

    def upload(self, release_id: int, path: Path) -> None:
        data = path.read_bytes()
        query = urllib.parse.quote(path.name)
        base = f"https://uploads.github.com/repos/{self.repo}/releases/{release_id}/assets?name={query}"
        status, _ = self.raw("POST", "", body=data, content_type="application/octet-stream", base=base)
        if status not in (200, 201):
            raise ReleaseError("asset upload failed")


def prepare_draft(api: GitHubApi, intent: ReleaseIntent, merge_sha: str, body: str) -> int:
    """Precheck the tag, then create the draft once or reuse the single existing draft (pruning Windows installers)."""
    existing_commit = api.tag_commit(intent.tag)
    if existing_commit is not None and existing_commit != merge_sha:
        raise ReleaseError("tag exists at another commit: failing closed")
    releases = api.releases_for_tag(intent.tag)
    if any(not release.get("draft") for release in releases):
        raise ReleaseError("a published release already exists for this tag")
    if len(releases) > 1:
        raise ReleaseError("more than one draft exists for this tag")
    if releases:
        release = releases[0]
        if release.get("prerelease") != intent.prerelease:
            raise ReleaseError("the draft prerelease flag does not match the tag form")
        release_id = int(release["id"])  # type: ignore[arg-type]
        for asset in release.get("assets", []):  # type: ignore[union-attr]
            if str(asset["name"]).endswith("-setup.exe"):
                api.call("DELETE", f"/releases/assets/{asset['id']}", ok=(204,))
        api.call("PATCH", f"/releases/{release_id}", {"body": body})
        return release_id
    created = api.call(
        "POST",
        "/releases",
        {
            "tag_name": intent.tag,
            "target_commitish": merge_sha,
            "name": f"{PRODUCT} {intent.version}",
            "body": body,
            "draft": True,
            "prerelease": intent.prerelease,
        },
        ok=(201,),
    )
    return int(created["id"])  # type: ignore[index,call-overload]


def attach_assets(api: GitHubApi, release_id: int, directory: Path) -> int:
    """Uploads the assets the draft lacks. An asset with the same name and a different digest fails."""
    release = api.call("GET", f"/releases/{release_id}")
    existing = {str(asset["name"]): asset for asset in release["assets"]}  # type: ignore[index,call-overload]
    uploaded = 0
    for path in sorted(entry for entry in directory.iterdir() if entry.is_file()):
        asset = existing.get(path.name)
        if asset is not None:
            if str(asset.get("digest") or "") != f"sha256:{sha256_file(path)}":
                raise ReleaseError("an attached asset has a different digest")
            continue
        api.upload(release_id, path)
        uploaded += 1
    return uploaded


def verify_draft(api: GitHubApi, intent: ReleaseIntent, merge_sha: str, assets_dir: Path) -> None:
    """Checks the draft against the tag, the commit and the local asset set. Fails with a fixed message."""
    fixed = "draft verification failed"
    if api.tag_commit(intent.tag) != merge_sha:
        raise ReleaseError(fixed)
    releases = api.releases_for_tag(intent.tag)
    if len(releases) != 1:
        raise ReleaseError(fixed)
    release = releases[0]
    if release.get("draft") is not True or release.get("prerelease") != intent.prerelease:
        raise ReleaseError(fixed)
    if release.get("target_commitish") != merge_sha:
        raise ReleaseError(fixed)
    local = {entry.name: entry for entry in assets_dir.iterdir() if entry.is_file()}
    assets = {str(asset["name"]): asset for asset in release.get("assets", [])}  # type: ignore[union-attr]
    if set(assets) != set(local) or set(local) != set(expected_asset_names(intent.version)):
        raise ReleaseError(fixed)
    for name, asset in assets.items():
        path = local[name]
        if str(asset.get("digest") or "") != f"sha256:{sha256_file(path)}" or asset.get("size") != path.stat().st_size:
            raise ReleaseError(fixed)


# ---------------------------------------------------------------------------
# Command line
# ---------------------------------------------------------------------------


def write_github_output(pairs: dict[str, str]) -> None:
    lines = "".join(f"{key}={value}\n" for key, value in pairs.items())
    target = os.environ.get("GITHUB_OUTPUT")
    if target:
        with open(target, "a", encoding="utf-8", newline="\n") as handle:
            handle.write(lines)
    else:
        sys.stdout.write(lines)


def changelog_body(changelog: Path, version: str) -> str:
    text = changelog.read_text(encoding="utf-8")
    match = re.search(rf"^## \[{re.escape(version)}\][^\n]*$", text, re.MULTILINE)
    if not match:
        raise ReleaseError(f"CHANGELOG.md has no section for {version}")
    following = re.search(r"^## \[", text[match.end():], re.MULTILINE)
    end = match.end() + following.start() if following else len(text)
    body = text[match.end():end].strip("\n")
    if not body.strip():
        raise ReleaseError(f"CHANGELOG.md section for {version} is empty")
    return body + "\n"


def token_from_env() -> str:
    token = os.environ.get("GITHUB_TOKEN", "")
    if not token:
        raise ReleaseError("GITHUB_TOKEN is not set")
    return token


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="JobniacFast release helpers")
    sub = parser.add_subparsers(dest="command", required=True)

    def add(name: str, help_text: str) -> argparse.ArgumentParser:
        return sub.add_parser(name, help=help_text)

    p = add("resolve-version", "check the three version fields and print the tag")
    p.add_argument("--repo-root", type=Path, default=REPO_ROOT)

    p = add("check-checkout", "HEAD must equal MERGE_SHA")
    p.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    p.add_argument("--merge-sha", required=True)

    p = add("stage-sidecar", "copy jobniac-cli to binaries/jobniac-cli-<triple>")
    p.add_argument("--target-dir", type=Path, required=True)
    p.add_argument("--binaries-dir", type=Path, required=True)

    p = add("notices", "generate THIRD-PARTY-NOTICES.txt and its manifest")
    p.add_argument("--out-dir", type=Path, required=True)
    p.add_argument("--pdfium-dir", type=Path, action="append", required=True)
    p.add_argument("--npm-json", type=Path, help="pnpm licenses list --prod --json output (else pnpm is run)")
    p.add_argument(
        "--draft-missing-texts",
        action="store_true",
        help="LOCAL DRAFTS ONLY: mark a licence text that is not supplied. The release workflow never passes it.",
    )

    p = add("normalize-sbom", "write a reproducible CycloneDX SBOM (serial from the seed, no timestamp)")
    p.add_argument("--in", dest="source", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--seed", required=True, help="for example v0.1.0|<MERGE_SHA>|<file role>")

    p = add("stage-payload", "copy the shipped files into a staging directory")
    p.add_argument("--target-dir", type=Path, required=True)
    p.add_argument("--platform", choices=PLATFORMS, required=True)
    p.add_argument("--pdfium-library", type=Path, required=True)
    p.add_argument("--notices", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)

    p = add("inventory", "hash and attribute the staged payload and reconcile the notices")
    p.add_argument("--payload", type=Path, required=True)
    p.add_argument("--notices-manifest", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--owners", type=Path, default=REPO_ROOT / "scripts/release/payload-owners.json")
    p.add_argument("--components", type=Path, default=REPO_ROOT / "scripts/release/notices/static-components.json")

    p = add("compare-installed", "compare the staged inventory with an installed tree")
    p.add_argument("--inventory", type=Path, required=True)
    p.add_argument("--installed", type=Path, required=True)

    p = add("leg-digests", "write leg-digests.txt for one platform leg")
    p.add_argument("--dir", type=Path, required=True)

    p = add("assemble", "assemble the release asset set from the leg artifacts")
    p.add_argument("--legs", type=Path, required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--version", required=True)

    p = add("exact-set", "the asset directory holds exactly the release set")
    p.add_argument("--dir", type=Path, required=True)
    p.add_argument("--version", required=True)

    p = add("changelog-body", "print the CHANGELOG.md section of a version")
    p.add_argument("--changelog", type=Path, default=REPO_ROOT / "CHANGELOG.md")
    p.add_argument("--version", required=True)
    p.add_argument("--out", type=Path, required=True)

    p = add("prepare", "read-only: resolve the version and check the checkout; print the outputs")
    p.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    p.add_argument("--merge-sha", required=True)

    p = add("draft", "create or reuse the single draft for the tag (write scope)")
    p.add_argument("--repo", required=True)
    p.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    p.add_argument("--merge-sha", required=True)
    p.add_argument("--changelog", type=Path, default=REPO_ROOT / "CHANGELOG.md")

    p = add("attach", "upload the missing assets to the draft; fail on a digest conflict")
    p.add_argument("--repo", required=True)
    p.add_argument("--release-id", type=int, required=True)
    p.add_argument("--dir", type=Path, required=True)

    p = add("verify-draft", "check the draft against the tag, the commit and the asset set")
    p.add_argument("--repo", required=True)
    p.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    p.add_argument("--merge-sha", required=True)
    p.add_argument("--dir", type=Path, required=True)
    return parser


def run_command(args: argparse.Namespace) -> int:
    command = args.command
    if command == "resolve-version":
        intent = resolve_version(args.repo_root)
        write_github_output({"version": intent.version, "tag": intent.tag, "prerelease": str(intent.prerelease).lower()})
    elif command == "check-checkout":
        check_checkout(args.repo_root, args.merge_sha)
        sys.stdout.write("checkout is MERGE_SHA\n")
    elif command == "stage-sidecar":
        destination = stage_sidecar(args.target_dir, args.binaries_dir, host_triple())
        sys.stdout.write(f"staged {destination.name}\n")
    elif command == "notices":
        manifest = build_notices(
            args.out_dir,
            list(args.pdfium_dir),
            run_cargo_about(),
            json.loads(args.npm_json.read_text(encoding="utf-8")) if args.npm_json else run_pnpm_licences(),
            REPO_ROOT / "scripts/release/notices/static-components.json",
            sysroot_path(),
            args.draft_missing_texts,
        )
        sys.stdout.write(f"notices written: {manifest['npm_packages']} npm packages\n")
    elif command == "normalize-sbom":
        normalize_sbom(args.source, args.out, args.seed)
    elif command == "stage-payload":
        suffix = ".exe" if args.platform == "windows-x64" else ""
        args.out.mkdir(parents=True, exist_ok=True)
        for name in (f"jobniac{suffix}", f"jobniac-cli{suffix}"):
            source = args.target_dir / name
            if not source.is_file():
                raise ReleaseError(f"{name} is missing from the target directory")
            shutil.copyfile(source, args.out / name)
        (args.out / "pdfium").mkdir(exist_ok=True)
        shutil.copyfile(args.pdfium_library, args.out / "pdfium" / args.pdfium_library.name)
        shutil.copyfile(args.notices, args.out / "THIRD-PARTY-NOTICES.txt")
    elif command == "inventory":
        inventory = build_inventory(args.payload, args.owners, args.components, args.notices_manifest)
        write_json(args.out, inventory)
        sys.stdout.write(f"inventory: {len(inventory['files'])} files reconciled\n")  # type: ignore[arg-type]
    elif command == "compare-installed":
        count = compare_installed(args.inventory, args.installed, INSTALLED_ONLY_ALLOWLIST)
        sys.stdout.write(f"installed tree matches the staged payload ({count} files)\n")
    elif command == "leg-digests":
        write_digest_manifest(args.dir, LEG_DIGESTS, set())
        sys.stdout.write(f"{LEG_DIGESTS} written\n")
    elif command == "assemble":
        names = assemble(args.legs, args.out, args.version)
        sys.stdout.write(f"assembled {len(names)} assets; SHA256SUMS verified\n")
    elif command == "exact-set":
        names = check_exact_asset_set(args.dir, args.version)
        sys.stdout.write(f"exact asset set: {len(names)} files\n")
    elif command == "changelog-body":
        args.out.write_bytes(changelog_body(args.changelog, args.version).encode("utf-8"))
    elif command == "prepare":
        intent = resolve_version(args.repo_root)
        check_checkout(args.repo_root, args.merge_sha)
        write_github_output({"version": intent.version, "tag": intent.tag, "prerelease": str(intent.prerelease).lower()})
    elif command == "draft":
        intent = resolve_version(args.repo_root)
        check_checkout(args.repo_root, args.merge_sha)
        body = changelog_body(args.changelog, intent.version)
        release_id = prepare_draft(GitHubApi(args.repo, token_from_env()), intent, args.merge_sha, body)
        write_github_output({"release_id": str(release_id)})
    elif command == "attach":
        uploaded = attach_assets(GitHubApi(args.repo, token_from_env()), args.release_id, args.dir)
        sys.stdout.write(f"attached {uploaded} missing assets; existing assets match their digests\n")
    elif command == "verify-draft":
        intent = resolve_version(args.repo_root)
        verify_draft(GitHubApi(args.repo, token_from_env()), intent, args.merge_sha, args.dir)
        sys.stdout.write("PASS: draft matches the tag, the commit and the asset set\n")
    else:
        raise ReleaseError("unknown command")
    return 0


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        return run_command(args)
    except ReleaseError as error:
        sys.stderr.write(f"FAIL: {error}\n")
        return 1


if __name__ == "__main__":
    sys.exit(main())
