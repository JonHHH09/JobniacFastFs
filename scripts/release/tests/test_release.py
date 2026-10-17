"""Tests for scripts/release/release.py and the release workflow rules.

Run from the repository root: `py -I -m unittest discover -s scripts/release/tests -t scripts/release/tests`
(or `python3 -I -m unittest ...` on Linux runners).
"""

from __future__ import annotations

import json
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
RELEASE_DIR = HERE.parent
REPO_ROOT = RELEASE_DIR.parents[1]
sys.path.insert(0, str(RELEASE_DIR))

import release  # noqa: E402

WORKFLOWS = REPO_ROOT / ".github" / "workflows"


class VersionRules(unittest.TestCase):
    def test_accepts_stable_and_dev_forms(self) -> None:
        stable = release.tag_for_version("1.2.3")
        self.assertEqual(stable.tag, "v1.2.3")
        self.assertFalse(stable.prerelease)
        dev = release.tag_for_version("1.2.3-dev.4")
        self.assertEqual(dev.tag, "v1.2.3-dev.4")
        self.assertTrue(dev.prerelease)

    def test_rejects_other_forms(self) -> None:
        for version in ("1.2", "1.2.3-alpha.1", "1.2.3-rc.1", "1.2.3-beta", "1.2.3+build.5", "1.2.3-dev", "v1.2.3"):
            with self.subTest(version=version), self.assertRaises(release.ReleaseError):
                release.tag_for_version(version)

    def test_repository_versions_are_equal_and_resolve(self) -> None:
        versions = release.read_versions(REPO_ROOT)
        self.assertEqual(len(set(versions.values())), 1, versions)
        intent = release.resolve_version(REPO_ROOT)
        self.assertEqual(intent.tag, f"v{intent.version}")

    def test_cargo_members_inherit_the_workspace_version(self) -> None:
        for manifest in [REPO_ROOT / "src-tauri" / "Cargo.toml", *sorted((REPO_ROOT / "crates").glob("*/Cargo.toml"))]:
            text = manifest.read_text(encoding="utf-8")
            with self.subTest(manifest=manifest.name):
                self.assertRegex(text, r"(?m)^version\.workspace = true$")

    def test_a_mismatched_field_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "src-tauri").mkdir()
            (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.1.0"\n', encoding="utf-8")
            (root / "src-tauri" / "tauri.conf.json").write_text('{"version": "0.1.1"}', encoding="utf-8")
            (root / "package.json").write_text('{"version": "0.1.0"}', encoding="utf-8")
            with self.assertRaises(release.ReleaseError):
                release.resolve_version(root)


class ChangelogBody(unittest.TestCase):
    def test_extracts_the_section_of_the_version(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "CHANGELOG.md"
            path.write_text("# Changelog\n\n## [Unreleased]\n\n## [1.0.0] - 2026-01-01\n\n### Added\n\n- a\n\n## [0.9.0] - 2025-01-01\n\n- old\n", encoding="utf-8")
            body = release.changelog_body(path, "1.0.0")
            self.assertIn("- a", body)
            self.assertNotIn("old", body)

    def test_missing_or_empty_section_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "CHANGELOG.md"
            path.write_text("## [1.0.0] - 2026-01-01\n\n## [1.0.1] - 2026-01-02\n\n- x\n", encoding="utf-8")
            with self.assertRaises(release.ReleaseError):
                release.changelog_body(path, "1.0.0")
            with self.assertRaises(release.ReleaseError):
                release.changelog_body(path, "2.0.0")

    def test_repository_changelog_has_the_current_version(self) -> None:
        intent = release.resolve_version(REPO_ROOT)
        self.assertTrue(release.changelog_body(REPO_ROOT / "CHANGELOG.md", intent.version).strip())


class Sha256Sums(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_write_then_verify_round_trip(self) -> None:
        (self.tmp / "b.bin").write_bytes(b"two")
        (self.tmp / "a.bin").write_bytes(b"one")
        manifest = release.write_sha256sums(self.tmp)
        text = manifest.read_bytes().decode("utf-8")
        self.assertTrue(text.endswith("\n"))
        self.assertNotIn("\r", text)
        self.assertEqual([line.split("  ")[1] for line in text.splitlines()], ["a.bin", "b.bin"])
        self.assertEqual(release.verify_sha256sums(self.tmp), 2)

    def test_tampered_file_fails_verification(self) -> None:
        (self.tmp / "a.bin").write_bytes(b"one")
        release.write_sha256sums(self.tmp)
        (self.tmp / "a.bin").write_bytes(b"changed")
        with self.assertRaises(release.ReleaseError):
            release.verify_sha256sums(self.tmp)

    def test_extra_or_missing_entry_fails(self) -> None:
        (self.tmp / "a.bin").write_bytes(b"one")
        release.write_sha256sums(self.tmp)
        (self.tmp / "extra.bin").write_bytes(b"x")
        with self.assertRaises(release.ReleaseError):
            release.verify_sha256sums(self.tmp)

    def test_crlf_is_refused(self) -> None:
        (self.tmp / "a.bin").write_bytes(b"one")
        manifest = release.write_sha256sums(self.tmp)
        manifest.write_bytes(manifest.read_bytes().replace(b"\n", b"\r\n"))
        with self.assertRaises(release.ReleaseError):
            release.verify_sha256sums(self.tmp)


class ExactAssetSet(unittest.TestCase):
    VERSION = "0.1.0"

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        for name in release.expected_asset_names(self.VERSION):
            (self.tmp / name).write_bytes(b"x")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_exact_set_passes(self) -> None:
        self.assertEqual(
            len(release.check_exact_asset_set(self.tmp, self.VERSION)),
            len(release.expected_asset_names(self.VERSION)),
        )

    def test_updater_artifacts_are_named_and_refused(self) -> None:
        for name in ("latest.json", "JobniacFast_0.1.0_x64-setup.exe.sig"):
            with self.subTest(name=name):
                path = self.tmp / name
                path.write_bytes(b"x")
                with self.assertRaises(release.ReleaseError) as context:
                    release.check_exact_asset_set(self.tmp, self.VERSION)
                self.assertIn("updater", str(context.exception))
                path.unlink()

    def test_missing_and_unexpected_files_fail(self) -> None:
        (self.tmp / "SHA256SUMS").unlink()
        with self.assertRaises(release.ReleaseError):
            release.check_exact_asset_set(self.tmp, self.VERSION)
        (self.tmp / "SHA256SUMS").write_bytes(b"x")
        (self.tmp / "unexpected.txt").write_bytes(b"x")
        with self.assertRaises(release.ReleaseError):
            release.check_exact_asset_set(self.tmp, self.VERSION)


class PayloadReconciliation(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.payload = self.tmp / "payload"
        self.payload.mkdir()
        self.components = self.tmp / "components.json"
        self.components.write_text(json.dumps({"components": [
            {"id": "pdfium", "payload_any": ["pdfium/pdfium.dll", "pdfium/libpdfium.so"]},
            {"id": "webview2", "external": True, "payload_any": []},
        ]}), encoding="utf-8")
        self.owners = self.tmp / "owners.json"
        self.owners.write_text(json.dumps({"rules": [
            {"path": "jobniac.exe", "owner": "crate:jobniac-desktop"},
            {"path": "pdfium/pdfium.dll", "owner": "static:pdfium"},
            {"path": "THIRD-PARTY-NOTICES.txt", "owner": "self:notices"},
        ]}), encoding="utf-8")
        self.notices = self.tmp / "notices.json"
        self.notices.write_text(json.dumps({"components": [{"id": "pdfium"}, {"id": "webview2"}]}), encoding="utf-8")
        (self.payload / "jobniac.exe").write_bytes(b"exe")
        (self.payload / "THIRD-PARTY-NOTICES.txt").write_bytes(b"n")
        (self.payload / "pdfium").mkdir()
        (self.payload / "pdfium" / "pdfium.dll").write_bytes(b"dll")

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def inventory(self) -> dict[str, object]:
        return release.build_inventory(self.payload, self.owners, self.components, self.notices)

    def test_matching_payload_reconciles(self) -> None:
        inventory = self.inventory()
        self.assertEqual(inventory["schema"], release.SCHEMA_INVENTORY)
        self.assertEqual([entry["path"] for entry in inventory["files"]], ["THIRD-PARTY-NOTICES.txt", "jobniac.exe", "pdfium/pdfium.dll"])

    def test_unowned_file_fails(self) -> None:
        (self.payload / "extra.dll").write_bytes(b"x")
        with self.assertRaises(release.ReleaseError):
            self.inventory()

    def test_notice_without_payload_fails(self) -> None:
        (self.payload / "pdfium" / "pdfium.dll").unlink()
        with self.assertRaises(release.ReleaseError):
            self.inventory()

    def test_component_without_notice_fails(self) -> None:
        self.notices.write_text(json.dumps({"components": [{"id": "webview2"}]}), encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.inventory()

    def test_installed_tree_comparison(self) -> None:
        inventory_path = self.tmp / "inventory.json"
        release.write_json(inventory_path, self.inventory())
        installed = self.tmp / "installed"
        shutil.copytree(self.payload, installed)
        (installed / "uninstall.exe").write_bytes(b"u")
        self.assertEqual(release.compare_installed(inventory_path, installed, release.INSTALLED_ONLY_ALLOWLIST), 3)
        (installed / "stray.dll").write_bytes(b"s")
        with self.assertRaises(release.ReleaseError):
            release.compare_installed(inventory_path, installed, release.INSTALLED_ONLY_ALLOWLIST)


class FakeTransport:
    """Serves canned GitHub API responses keyed by (method, path suffix)."""

    def __init__(self, routes: dict[tuple[str, str], tuple[int, object]]) -> None:
        self.routes = routes
        self.calls: list[tuple[str, str]] = []

    def __call__(self, method: str, url: str, token: str, body: bytes | None, content_type: str) -> tuple[int, bytes]:
        path = "/" + url.split("/repos/", 1)[1].split("/", 2)[2]
        self.calls.append((method, path))
        for (route_method, suffix), (status, payload) in self.routes.items():
            if route_method == method and path.startswith(suffix):
                if isinstance(payload, bytes):
                    return status, payload
                return status, json.dumps(payload).encode("utf-8")
        return 404, b'{"message": "Not Found"}'


class DraftVerification(unittest.TestCase):
    SHA = "a" * 40
    VERSION = "0.1.0"

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.assets = self.tmp / "assets"
        self.assets.mkdir()
        for name in release.expected_asset_names(self.VERSION):
            (self.assets / name).write_bytes(name.encode("utf-8"))

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def release_payload(self, **overrides: object) -> dict[str, object]:
        assets = []
        for index, name in enumerate(release.expected_asset_names(self.VERSION)):
            path = self.assets / name
            assets.append({
                "id": index,
                "name": name,
                "size": path.stat().st_size,
                "digest": f"sha256:{release.sha256_file(path)}",
            })
        payload: dict[str, object] = {
            "id": 7,
            "tag_name": f"v{self.VERSION}",
            "draft": True,
            "prerelease": False,
            "target_commitish": self.SHA,
            "assets": assets,
        }
        payload.update(overrides)
        return payload

    def api(self, release_payload: dict[str, object], tag_sha: str | None = None) -> release.GitHubApi:
        tag_sha = tag_sha or self.SHA
        routes: dict[tuple[str, str], tuple[int, object]] = {
            ("GET", "/git/ref/tags/"): (200, {"object": {"type": "commit", "sha": tag_sha}}),
            ("GET", "/releases?"): (200, [release_payload]),
        }
        return release.GitHubApi("owner/repo", "token", FakeTransport(routes))

    def intent(self) -> release.ReleaseIntent:
        return release.tag_for_version(self.VERSION)

    def test_matching_draft_passes(self) -> None:
        release.verify_draft(self.api(self.release_payload()), self.intent(), self.SHA, self.assets)

    def test_wrong_commit_fails(self) -> None:
        other = "b" * 40
        with self.assertRaises(release.ReleaseError):
            release.verify_draft(self.api(self.release_payload(), tag_sha=other), self.intent(), self.SHA, self.assets)

    def test_published_or_wrong_flags_fail(self) -> None:
        for overrides in ({"draft": False}, {"prerelease": True}, {"target_commitish": "c" * 40}):
            with self.subTest(overrides=overrides), self.assertRaises(release.ReleaseError):
                release.verify_draft(self.api(self.release_payload(**overrides)), self.intent(), self.SHA, self.assets)

    def test_asset_digest_or_set_mismatch_fails(self) -> None:
        payload = self.release_payload()
        payload["assets"][0]["digest"] = "sha256:" + "0" * 64  # type: ignore[index]
        with self.assertRaises(release.ReleaseError):
            release.verify_draft(self.api(payload), self.intent(), self.SHA, self.assets)
        payload = self.release_payload()
        payload["assets"].pop()  # type: ignore[attr-defined]
        with self.assertRaises(release.ReleaseError):
            release.verify_draft(self.api(payload), self.intent(), self.SHA, self.assets)

    def test_duplicate_json_keys_are_rejected(self) -> None:
        routes = {("GET", "/git/ref/tags/"): (200, b'{"object": {"type": "commit", "sha": "x"}, "object": {}}')}
        api = release.GitHubApi("owner/repo", "token", FakeTransport(routes))
        with self.assertRaises(release.ReleaseError):
            api.call("GET", "/git/ref/tags/v0.1.0")

    def test_oversized_response_is_rejected(self) -> None:
        routes = {("GET", "/git/ref/tags/"): (200, b" " * (release.API_MAX_BYTES + 10))}
        api = release.GitHubApi("owner/repo", "token", FakeTransport(routes))
        with self.assertRaises(release.ReleaseError):
            api.call("GET", "/git/ref/tags/v0.1.0")


class PrepareDraft(unittest.TestCase):
    SHA = "a" * 40

    def test_existing_published_release_refuses(self) -> None:
        routes = {("GET", "/releases?"): (200, [{"id": 1, "tag_name": "v0.1.0", "draft": False, "prerelease": False, "assets": []}])}
        api = release.GitHubApi("owner/repo", "token", FakeTransport(routes))
        with self.assertRaises(release.ReleaseError):
            release.prepare_draft(api, release.tag_for_version("0.1.0"), self.SHA, "body")

    def test_new_draft_is_created_once(self) -> None:
        transport = FakeTransport({
            ("GET", "/releases?"): (200, []),
            ("POST", "/releases"): (201, {"id": 42}),
        })
        api = release.GitHubApi("owner/repo", "token", transport)
        self.assertEqual(release.prepare_draft(api, release.tag_for_version("0.1.0"), self.SHA, "body"), 42)
        self.assertEqual(transport.calls.count(("POST", "/releases")), 1)


class SbomNormalisation(unittest.TestCase):
    def test_serial_and_timestamp_are_replaced_and_order_is_stable(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        try:
            first = {"bomFormat": "CycloneDX", "specVersion": "1.5", "serialNumber": "urn:uuid:random-1",
                     "metadata": {"timestamp": "2026-01-01T00:00:00Z"},
                     "components": [{"name": "b"}, {"name": "a"}]}
            second = {"bomFormat": "CycloneDX", "specVersion": "1.5", "serialNumber": "urn:uuid:random-2",
                      "metadata": {"timestamp": "2026-06-01T12:00:00Z"},
                      "components": [{"name": "a"}, {"name": "b"}]}
            (tmp / "one.json").write_text(json.dumps(first), encoding="utf-8")
            (tmp / "two.json").write_text(json.dumps(second), encoding="utf-8")
            release.normalize_sbom(tmp / "one.json", tmp / "one.out", "v0.1.0|abc|desktop")
            release.normalize_sbom(tmp / "two.json", tmp / "two.out", "v0.1.0|abc|desktop")
            self.assertEqual((tmp / "one.out").read_bytes(), (tmp / "two.out").read_bytes())
            text = (tmp / "one.out").read_text(encoding="utf-8")
            self.assertNotIn("timestamp", text)
            self.assertNotIn("random-1", text)
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_a_non_cyclonedx_input_is_refused(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        try:
            (tmp / "x.json").write_text('{"bomFormat": "SPDX"}', encoding="utf-8")
            with self.assertRaises(release.ReleaseError):
                release.normalize_sbom(tmp / "x.json", tmp / "y.json", "seed")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


class LegAssembly(unittest.TestCase):
    def make_leg(self, root: Path, name: str, files: dict[str, bytes]) -> None:
        leg = root / name
        leg.mkdir(parents=True)
        for file_name, data in files.items():
            (leg / file_name).write_bytes(data)
        release.write_digest_manifest(leg, release.LEG_DIGESTS, set())

    def test_shared_files_must_agree_between_legs(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        try:
            self.make_leg(tmp / "legs", "a", {"THIRD-PARTY-NOTICES.txt": b"same"})
            self.make_leg(tmp / "legs", "b", {"THIRD-PARTY-NOTICES.txt": b"different"})
            with self.assertRaises(release.ReleaseError):
                release.assemble(tmp / "legs", tmp / "out", "0.1.0")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    def test_a_leg_with_a_changed_file_is_refused(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        try:
            self.make_leg(tmp / "legs", "a", {"THIRD-PARTY-NOTICES.txt": b"same"})
            (tmp / "legs" / "a" / "THIRD-PARTY-NOTICES.txt").write_bytes(b"changed after the digest")
            with self.assertRaises(release.ReleaseError):
                release.assemble(tmp / "legs", tmp / "out", "0.1.0")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


class NoticeSources(unittest.TestCase):
    def test_static_components_have_text_sources_or_are_gated(self) -> None:
        components = json.loads((RELEASE_DIR / "notices" / "static-components.json").read_text(encoding="utf-8"))["components"]
        ids = {component["id"] for component in components}
        self.assertEqual(ids, {"pdfium", "adobe-core14-afm", "rust-toolchain", "webview2"})
        self.assertTrue((REPO_ROOT / "crates" / "pdf" / "src" / "metrics" / "NOTICE-Adobe-AFM.txt").is_file())

    def test_npm_overrides_are_verbatim_files_with_a_source_note(self) -> None:
        override = RELEASE_DIR / "notices" / "npm-overrides" / "react-remove-scroll-bar@2.3.8"
        self.assertTrue((override / "LICENSE").is_file())
        self.assertTrue((override / "SOURCE.txt").is_file())


def merge_patch(target: object, patch: object) -> object:
    """JSON Merge Patch (RFC 7396), which Tauri uses for platform and --config files: objects merge, arrays replace."""
    if not isinstance(patch, dict):
        return patch
    result = dict(target) if isinstance(target, dict) else {}
    for key, value in patch.items():
        if value is None:
            result.pop(key, None)
        else:
            result[key] = merge_patch(result.get(key), value)
    return result


class BundleConfig(unittest.TestCase):
    """The default checkout builds without staged files; only the release build adds them."""

    STAGED_EXTERNAL_BIN = ["binaries/jobniac-cli"]
    STAGED_NOTICES = "resources/THIRD-PARTY-NOTICES.txt"
    PDFIUM_WINDOWS = "../vendor/pdfium/windows-x64/pdfium.dll"
    PDFIUM_LINUX = "../vendor/pdfium/linux-x64/libpdfium.so"

    def load(self, name: str) -> dict[str, object]:
        return json.loads((REPO_ROOT / "src-tauri" / name).read_text(encoding="utf-8"))

    def effective(self, platform_file: str) -> dict[str, object]:
        merged = merge_patch(self.load("tauri.conf.json"), self.load(platform_file))
        return merge_patch(merged, self.load("tauri.release.conf.json"))  # type: ignore[return-value]

    def test_default_config_references_no_staged_file(self) -> None:
        bundle = self.load("tauri.conf.json")["bundle"]
        self.assertNotIn("externalBin", bundle)
        self.assertNotIn(self.STAGED_NOTICES, bundle.get("resources", {}))

    def test_default_platform_configs_only_map_pdfium(self) -> None:
        self.assertEqual(
            self.load("tauri.windows.conf.json")["bundle"]["resources"],
            {self.PDFIUM_WINDOWS: "pdfium/pdfium.dll"},
        )
        self.assertEqual(
            self.load("tauri.linux.conf.json")["bundle"]["resources"],
            {self.PDFIUM_LINUX: "pdfium/libpdfium.so"},
        )

    def test_release_config_adds_sidecar_and_notices_and_keeps_pdfium(self) -> None:
        for platform_file, pdfium in (
            ("tauri.windows.conf.json", self.PDFIUM_WINDOWS),
            ("tauri.linux.conf.json", self.PDFIUM_LINUX),
        ):
            with self.subTest(platform=platform_file):
                bundle = self.effective(platform_file)["bundle"]
                self.assertEqual(bundle["externalBin"], self.STAGED_EXTERNAL_BIN)
                self.assertEqual(
                    bundle["resources"],
                    {
                        self.STAGED_NOTICES: "THIRD-PARTY-NOTICES.txt",
                        pdfium: self.load(platform_file)["bundle"]["resources"][pdfium],
                    },
                )

    def test_release_workflow_passes_the_release_config_to_tauri_action(self) -> None:
        text = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        self.assertRegex(text, r"args: --bundles \$\{\{ matrix\.bundles \}\} --config src-tauri/tauri\.release\.conf\.json")

    def test_no_workflow_or_script_uses_a_debug_only_config_override(self) -> None:
        for path in list(WORKFLOWS.glob("*.yml")) + [RELEASE_DIR / "release.py"]:
            with self.subTest(file=path.name):
                self.assertNotIn("tauri.e2e.conf.json", path.read_text(encoding="utf-8"))


class WorkflowRules(unittest.TestCase):
    """Pins, permissions and the publication guard for every release and CI workflow."""

    PUBLICATION_PATTERNS = (
        re.compile(r"gh\s+release\s+(edit|publish)"),
        re.compile(r"draft\s*:\s*false", re.IGNORECASE),
        re.compile(r"\"draft\"\s*:\s*false", re.IGNORECASE),
        re.compile(r"\bpublish_release\b"),
    )

    def workflow_files(self) -> list[Path]:
        # ci.yml is outside this release part; its pins are tracked as an open item in the report.
        return [WORKFLOWS / "release.yml"]

    def code_lines(self, path: Path) -> list[str]:
        """Non-comment lines, so the explanations in a workflow do not trip the guard."""
        lines = []
        for line in path.read_text(encoding="utf-8").splitlines():
            stripped = line.split(" #", 1)[0] if not line.lstrip().startswith("#") else ""
            if stripped.strip():
                lines.append(stripped)
        return lines

    def test_no_workflow_publishes_or_edits_a_release(self) -> None:
        for path in self.workflow_files():
            for line in self.code_lines(path):
                for pattern in self.PUBLICATION_PATTERNS:
                    with self.subTest(file=path.name, line=line):
                        self.assertIsNone(pattern.search(line))

    def test_every_action_is_pinned_to_a_full_sha_with_a_version_comment(self) -> None:
        for path in self.workflow_files():
            for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                match = re.search(r"^\s*(?:-\s*)?uses:\s*(\S+)", line)
                if not match:
                    continue
                reference = match.group(1)
                if reference.startswith("./") or reference.startswith("docker://"):
                    continue
                with self.subTest(file=path.name, line=number):
                    self.assertRegex(reference, r"@[0-9a-f]{40}$")
                    self.assertRegex(line, r"#\s*v?\d", "the pinned version must be in a comment")

    def test_checkouts_do_not_persist_credentials(self) -> None:
        for path in self.workflow_files():
            text = path.read_text(encoding="utf-8")
            for match in re.finditer(r"uses: actions/checkout@[^\n]*\n((?:\s{4,}[^\n]*\n|\s*\n)*)", text):
                with self.subTest(file=path.name):
                    self.assertRegex(match.group(1), r"persist-credentials:\s*false")

    def test_no_pull_request_target(self) -> None:
        for path in self.workflow_files():
            self.assertNotIn("pull_request_target", path.read_text(encoding="utf-8"), path.name)

    def test_top_level_permissions_are_read_only(self) -> None:
        for path in self.workflow_files():
            text = path.read_text(encoding="utf-8")
            with self.subTest(file=path.name):
                self.assertRegex(text, r"(?m)^permissions:\n\s+contents: read$")


if __name__ == "__main__":
    unittest.main()
