"""Tests for scripts/release/release.py and the release workflow rules.

Run from the repository root: `py -I -m unittest discover -s scripts/release/tests -t scripts/release/tests`
(or `python3 -I -m unittest ...` on Linux runners).
"""

from __future__ import annotations

import contextlib
import io
import json
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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


class FakeRepo:
    """A throwaway tree with the three version files and a changelog."""

    def __init__(self, version: str = "0.1.0", changelog: str | None = None) -> None:
        self.root = Path(tempfile.mkdtemp())
        (self.root / "src-tauri").mkdir()
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["a"]\n\n[workspace.package]\n'
            f'version = "{version}"\nedition = "2024"\n\n[workspace.dependencies]\nversion = "9.9.9"\n',
            encoding="utf-8",
        )
        (self.root / "src-tauri" / "tauri.conf.json").write_text(
            f'{{\n  "productName": "X",\n  "version": "{version}",\n  "identifier": "a.b"\n}}\n', encoding="utf-8"
        )
        (self.root / "package.json").write_text(
            f'{{\n  "name": "x",\n  "version": "{version}",\n  "dependencies": {{"v": "1"}}\n}}\n', encoding="utf-8"
        )
        self.changelog = self.root / "CHANGELOG.md"
        self.changelog.write_text(
            changelog
            if changelog is not None
            else "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- a thing\n\n## [0.1.0] - 2026-01-01\n\n- first\n",
            encoding="utf-8",
        )

    def cleanup(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)


class VersionBump(unittest.TestCase):
    def setUp(self) -> None:
        self.repo = FakeRepo()
        self.addCleanup(self.repo.cleanup)

    def test_bump_updates_all_three_files_and_dates_the_changelog(self) -> None:
        release.bump_version(self.repo.root, "0.2.0", "2026-02-03")
        self.assertEqual(set(release.read_versions(self.repo.root).values()), {"0.2.0"})
        text = self.repo.changelog.read_text(encoding="utf-8")
        self.assertIn("## [Unreleased]\n\n## [0.2.0] - 2026-02-03\n\n### Added\n\n- a thing", text)
        self.assertIn("## [0.1.0] - 2026-01-01", text)
        self.assertEqual(release.changelog_body(self.repo.changelog, "0.2.0"), "### Added\n\n- a thing\n")

    def test_bump_touches_only_the_package_version_lines(self) -> None:
        release.bump_version(self.repo.root, "0.2.0", "2026-02-03")
        cargo = (self.repo.root / "Cargo.toml").read_text(encoding="utf-8")
        self.assertIn('[workspace.package]\nversion = "0.2.0"', cargo)
        self.assertIn('[workspace.dependencies]\nversion = "9.9.9"', cargo)
        package = (self.repo.root / "package.json").read_text(encoding="utf-8")
        self.assertIn('"version": "0.2.0",\n  "dependencies"', package)

    def test_bump_accepts_a_dev_version_and_rejects_other_forms(self) -> None:
        release.bump_version(self.repo.root, "0.2.0-dev.1", "2026-02-03")
        self.assertEqual(release.resolve_version(self.repo.root).tag, "v0.2.0-dev.1")
        for version in ("1.0", "1.0.0-rc.1", "v1.0.0"):
            with self.subTest(version=version), self.assertRaises(release.ReleaseError):
                release.bump_version(self.repo.root, version, "2026-02-03")

    def test_a_refused_bump_leaves_every_file_unchanged(self) -> None:
        names = ["Cargo.toml", "src-tauri/tauri.conf.json", "package.json", "CHANGELOG.md"]
        before = {name: (self.repo.root / name).read_bytes() for name in names}
        for version in ("0.1.0", "0.1.0-dev.x"):
            with self.subTest(version=version), self.assertRaises(release.ReleaseError):
                release.bump_version(self.repo.root, version, "2026-02-03")
        self.repo.changelog.write_text("# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-01-01\n\n- first\n", encoding="utf-8")
        before["CHANGELOG.md"] = self.repo.changelog.read_bytes()
        with self.assertRaises(release.ReleaseError):
            release.bump_version(self.repo.root, "0.2.0", "2026-02-03")
        self.assertEqual(before, {name: (self.repo.root / name).read_bytes() for name in names})

    def test_an_existing_version_section_refuses(self) -> None:
        self.repo.changelog.write_text("## [Unreleased]\n\n- x\n\n## [0.2.0] - 2026-01-02\n\n- y\n", encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            release.bump_version(self.repo.root, "0.2.0", "2026-02-03")

    def test_link_references_follow_the_new_version(self) -> None:
        self.repo.changelog.write_text(
            "## [Unreleased]\n\n- x\n\n## [0.1.0] - 2026-01-01\n\n- y\n\n"
            "[Unreleased]: https://example.org/r/compare/v0.1.0...HEAD\n"
            "[0.1.0]: https://example.org/r/releases/tag/v0.1.0\n",
            encoding="utf-8",
        )
        release.bump_version(self.repo.root, "0.2.0", "2026-02-03")
        text = self.repo.changelog.read_text(encoding="utf-8")
        self.assertIn("[Unreleased]: https://example.org/r/compare/v0.2.0...HEAD\n", text)
        self.assertIn("[0.2.0]: https://example.org/r/compare/v0.1.0...v0.2.0\n", text)
        self.assertIn("[0.1.0]: https://example.org/r/releases/tag/v0.1.0", text)

    def test_refresh_lockfile_runs_cargo_update_for_the_workspace(self) -> None:
        seen: list[list[str]] = []

        def fake_run(command: list[str], **_: object) -> object:
            seen.append(command)
            return type("Done", (), {"returncode": 0})()

        if release.shutil.which("cargo") is None:
            self.assertFalse(release.refresh_lockfile(self.repo.root, fake_run))
        else:
            self.assertTrue(release.refresh_lockfile(self.repo.root, fake_run))
            self.assertEqual(seen, [["cargo", "update", "--workspace"]])

    def test_the_command_line_bumps_and_names_the_cargo_step(self) -> None:
        with contextlib.redirect_stdout(io.StringIO()) as captured:
            status = release.main(
                ["bump", "0.3.0", "--repo-root", str(self.repo.root), "--date", "2026-03-04", "--no-cargo"]
            )
        self.assertEqual(status, 0)
        self.assertIn("cargo update", captured.getvalue())
        self.assertEqual(release.resolve_version(self.repo.root).version, "0.3.0")


class CheckVersions(unittest.TestCase):
    def test_agreeing_versions_with_a_version_section_pass(self) -> None:
        repo = FakeRepo(changelog="## [Unreleased]\n\n## [0.1.0] - 2026-01-01\n\n- first\n")
        self.addCleanup(repo.cleanup)
        self.assertEqual(release.check_versions(repo.root), "0.1.0")

    def test_notes_under_unreleased_pass_without_a_version_section(self) -> None:
        repo = FakeRepo(changelog="## [Unreleased]\n\n- pending\n")
        self.addCleanup(repo.cleanup)
        self.assertEqual(release.check_versions(repo.root), "0.1.0")

    def test_no_notes_anywhere_fails(self) -> None:
        for text in ("## [Unreleased]\n\n## [0.0.9] - 2026-01-01\n\n- old\n", "## [Unreleased]\n\n## [0.1.0] - 2026-01-01\n\n"):
            repo = FakeRepo(changelog=text)
            self.addCleanup(repo.cleanup)
            with self.subTest(text=text), self.assertRaises(release.ReleaseError):
                release.check_versions(repo.root)

    def test_differing_versions_fail(self) -> None:
        repo = FakeRepo()
        self.addCleanup(repo.cleanup)
        (repo.root / "package.json").write_text('{"version": "0.1.1"}', encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            release.check_versions(repo.root)

    def test_the_repository_itself_passes(self) -> None:
        release.check_versions(REPO_ROOT)

    def test_the_command_line_reports_a_failure_with_status_one(self) -> None:
        repo = FakeRepo(changelog="## [Unreleased]\n")
        self.addCleanup(repo.cleanup)
        with contextlib.redirect_stderr(io.StringIO()) as captured:
            self.assertEqual(release.main(["check-versions", "--repo-root", str(repo.root)]), 1)
        self.assertIn("FAIL:", captured.getvalue())


class UpdaterConfigGate(unittest.TestCase):
    def config(self, pubkey: str, **overrides: object) -> Path:
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, True)
        (root / "src-tauri").mkdir()
        data: dict[str, object] = {
            "bundle": {"createUpdaterArtifacts": True},
            "plugins": {"updater": {"pubkey": pubkey, "endpoints": [release.UPDATER_ENDPOINT]}},
        }
        data.update(overrides)
        (root / "src-tauri" / "tauri.release.conf.json").write_text(json.dumps(data), encoding="utf-8")
        return root

    def test_the_committed_release_config_has_a_real_key(self) -> None:
        release.check_updater_config(REPO_ROOT)

    def test_the_placeholder_message_names_the_key(self) -> None:
        with self.assertRaises(release.ReleaseError) as context:
            release.check_updater_config(self.config(release.UPDATER_PUBKEY_PLACEHOLDER))
        self.assertIn("placeholder", str(context.exception))

    def test_the_placeholder_or_an_empty_key_is_refused(self) -> None:
        for pubkey in (release.UPDATER_PUBKEY_PLACEHOLDER, "", "  "):
            with self.subTest(pubkey=pubkey), self.assertRaises(release.ReleaseError):
                release.check_updater_config(self.config(pubkey))

    def test_a_real_key_passes(self) -> None:
        release.check_updater_config(self.config("dW50cnVzdGVkIGNvbW1lbnQ6IHRlc3Q="))

    def test_updater_artifacts_must_be_on_and_the_endpoint_fixed(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.check_updater_config(self.config("k", bundle={"createUpdaterArtifacts": False}))
        root = self.config("k", plugins={"updater": {"pubkey": "k", "endpoints": ["https://example.org/latest.json"]}})
        with self.assertRaises(release.ReleaseError):
            release.check_updater_config(root)

    def test_prepare_and_draft_refuse_the_placeholder_before_anything_else(self) -> None:
        repo = FakeRepo()
        self.addCleanup(repo.cleanup)
        (repo.root / "src-tauri" / "tauri.release.conf.json").write_text(
            json.dumps({
                "bundle": {"createUpdaterArtifacts": True},
                "plugins": {"updater": {"pubkey": release.UPDATER_PUBKEY_PLACEHOLDER, "endpoints": [release.UPDATER_ENDPOINT]}},
            }),
            encoding="utf-8",
        )
        commands = (
            ["check-updater-config"],
            ["prepare", "--merge-sha", "a" * 40],
            ["draft", "--repo", "o/r", "--merge-sha", "a" * 40, "--version", "0.1.0"],
        )
        for command in commands:
            with self.subTest(command=command[0]), contextlib.redirect_stderr(io.StringIO()) as captured:
                self.assertEqual(release.main([*command, "--repo-root", str(repo.root)]), 1)
                self.assertIn("placeholder", captured.getvalue())


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

    def test_the_set_holds_the_signatures_of_the_installer_and_the_appimage(self) -> None:
        names = release.expected_asset_names(self.VERSION)
        self.assertIn("JobniacFast_0.1.0_x64-setup.exe.sig", names)
        self.assertIn("JobniacFast_0.1.0_amd64.AppImage.sig", names)
        self.assertIn("JobniacFast_0.1.0_aarch64.app.tar.gz.sig", names)
        self.assertNotIn("latest.json", names)

    def test_the_set_holds_the_macos_dmg_archive_and_inventory(self) -> None:
        names = release.expected_asset_names(self.VERSION)
        self.assertIn("JobniacFast_0.1.0_aarch64.dmg", names)
        self.assertIn("JobniacFast_0.1.0_aarch64.app.tar.gz", names)
        self.assertIn("JobniacFast-0.1.0-macos-arm64.payload-inventory.json", names)

    def test_a_missing_signature_fails(self) -> None:
        for name in (
            "JobniacFast_0.1.0_x64-setup.exe.sig",
            "JobniacFast_0.1.0_amd64.AppImage.sig",
            "JobniacFast_0.1.0_aarch64.app.tar.gz.sig",
        ):
            with self.subTest(name=name):
                (self.tmp / name).unlink()
                with self.assertRaises(release.ReleaseError):
                    release.check_exact_asset_set(self.tmp, self.VERSION)
                (self.tmp / name).write_bytes(b"x")

    def test_latest_json_is_not_part_of_the_local_set(self) -> None:
        (self.tmp / "latest.json").write_bytes(b"{}")
        with self.assertRaises(release.ReleaseError) as context:
            release.check_exact_asset_set(self.tmp, self.VERSION)
        self.assertIn("latest.json", str(context.exception))

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
        with self.assertRaises(release.ReleaseError) as caught:
            release.compare_installed(inventory_path, installed, release.INSTALLED_ONLY_ALLOWLIST)
        self.assertEqual(str(caught.exception), "unexpected file in the installed tree: stray.dll")

    def test_installed_tree_failures_name_the_relative_path(self) -> None:
        inventory_path = self.tmp / "inventory.json"
        release.write_json(inventory_path, self.inventory())
        installed = self.tmp / "installed"
        shutil.copytree(self.payload, installed)
        allowlist = release.INSTALLED_ONLY_ALLOWLIST
        (installed / "pdfium" / "pdfium.dll").write_bytes(b"changed")
        with self.assertRaises(release.ReleaseError) as caught:
            release.compare_installed(inventory_path, installed, allowlist)
        self.assertEqual(str(caught.exception), "installed file differs from the staged payload: pdfium/pdfium.dll")
        (installed / "pdfium" / "pdfium.dll").unlink()
        with self.assertRaises(release.ReleaseError) as caught:
            release.compare_installed(inventory_path, installed, allowlist)
        self.assertEqual(str(caught.exception), "missing file in the installed tree: pdfium/pdfium.dll")
        self.assertNotIn(str(self.tmp), str(caught.exception))


class BinaryPubkey(unittest.TestCase):
    """The shipped main binary embeds the updater public key of the release config exactly once."""

    PUBKEY = "dGVzdC1wdWJsaWMta2V5LWZvci10aGUtdW5pdC10ZXN0cw=="

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = Path(self._tmp.name)
        (self.tmp / "src-tauri").mkdir()
        config = {
            "bundle": {"createUpdaterArtifacts": True},
            "plugins": {"updater": {"pubkey": self.PUBKEY, "endpoints": [release.UPDATER_ENDPOINT]}},
        }
        (self.tmp / "src-tauri" / "tauri.release.conf.json").write_text(json.dumps(config), encoding="utf-8")

    def check(self, content: bytes) -> None:
        binary = self.tmp / "app"
        binary.write_bytes(content)
        release.check_binary_pubkey(binary, self.tmp)

    def test_one_occurrence_passes(self) -> None:
        self.check(b"head" + self.PUBKEY.encode() + b"tail")

    def test_zero_occurrences_fail(self) -> None:
        with self.assertRaises(release.ReleaseError) as caught:
            self.check(b"no key here")
        self.assertIn("0 times", str(caught.exception))

    def test_two_occurrences_fail(self) -> None:
        with self.assertRaises(release.ReleaseError) as caught:
            self.check(self.PUBKEY.encode() + b"mid" + self.PUBKEY.encode())
        self.assertIn("2 times", str(caught.exception))
        self.assertNotIn(str(self.tmp), str(caught.exception))

    def test_the_command_reports_the_result(self) -> None:
        binary = self.tmp / "app"
        binary.write_bytes(b"x" + self.PUBKEY.encode())
        self.assertEqual(release.main(["check-binary-pubkey", "--binary", str(binary), "--repo-root", str(self.tmp)]), 0)
        binary.write_bytes(b"x")
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(release.main(["check-binary-pubkey", "--binary", str(binary), "--repo-root", str(self.tmp)]), 1)


class PayloadStaging(unittest.TestCase):
    """The staged files match what the bundles ship: the sidecar mode and the Tauri bundle type."""

    MARKER = release.TAURI_BUNDLE_TYPE_MARKER

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.target = self.tmp / "target"
        self.target.mkdir()

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    @unittest.skipIf(sys.platform == "win32", "POSIX mode bits")
    def test_sidecar_keeps_the_executable_bit(self) -> None:
        source = self.target / "jobniac-cli"
        source.write_bytes(b"cli")
        source.chmod(0o755)
        staged = release.stage_sidecar(self.target, self.tmp / "binaries", "x86_64-unknown-linux-gnu")
        self.assertEqual(staged.stat().st_mode & 0o777, 0o755)

    def stage_windows(self, desktop: bytes) -> int:
        (self.target / "jobniac.exe").write_bytes(desktop)
        (self.target / "jobniac-cli.exe").write_bytes(b"cli")
        (self.tmp / "pdfium.dll").write_bytes(b"dll")
        (self.tmp / "notices.txt").write_bytes(b"n")
        return release.main([
            "stage-payload", "--target-dir", str(self.target), "--platform", "windows-x64",
            "--pdfium-library", str(self.tmp / "pdfium.dll"), "--notices", str(self.tmp / "notices.txt"),
            "--out", str(self.tmp / "payload"),
        ])

    def test_windows_payload_gets_the_nsis_bundle_type(self) -> None:
        # The constant table of all types stays as it is; only the UNK variable is patched.
        table = self.MARKER + b"DEB" + self.MARKER + b"NSS"
        self.assertEqual(self.stage_windows(b"head" + table + b"mid" + self.MARKER + b"UNK" + b"tail"), 0)
        staged = (self.tmp / "payload" / "jobniac.exe").read_bytes()
        self.assertEqual(staged, b"head" + table + b"mid" + self.MARKER + b"NSS" + b"tail")
        self.assertIn(self.MARKER + b"UNK", (self.target / "jobniac.exe").read_bytes())

    def test_windows_payload_without_a_single_marker_fails(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(self.stage_windows(b"no marker"), 1)


class SmokeFailureText(unittest.TestCase):
    """A smoke FAIL line names the cause and never a path."""

    def test_fixed_messages_are_shown_and_paths_are_not(self) -> None:
        import clean_machine_smoke as smoke

        self.assertEqual(smoke.describe(smoke.SmokeFailure("--help failed")), "SmokeFailure: --help failed")
        self.assertEqual(smoke.describe(release.ReleaseError("installed file differs")), "ReleaseError: installed file differs")
        error = PermissionError(13, "Permission denied", "/private/path/jobniac-cli")
        self.assertEqual(smoke.describe(error), "PermissionError: Permission denied")
        self.assertEqual(smoke.describe(KeyError("/private/path")), "KeyError")


class SmokeEnvironment(unittest.TestCase):
    """The smoke children get a database under the work directory and no other JOBNIAC_* variable."""

    def test_database_is_isolated_under_the_work_directory(self) -> None:
        import clean_machine_smoke as smoke

        with tempfile.TemporaryDirectory() as name:
            work = Path(name)
            with mock.patch.dict("os.environ", {"JOBNIAC_DB_PATH": "elsewhere", "JOBNIAC_PDFIUM_DIR": "x", "KEEP_ME": "1"}):
                env = smoke.smoke_env(work, JOBNIAC_SMOKE_QUIT_AFTER_LOAD="1")
            self.assertEqual(env["JOBNIAC_DB_PATH"], str(work.resolve() / "data" / "smoke.sqlite3"))
            self.assertTrue(Path(env["JOBNIAC_DB_PATH"]).is_absolute())
            self.assertEqual(env["JOBNIAC_SMOKE_QUIT_AFTER_LOAD"], "1")
            self.assertNotIn("JOBNIAC_PDFIUM_DIR", env)
            self.assertEqual(env["KEEP_ME"], "1")


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
        self.latest = self.tmp / "latest.json"
        self.write_latest()

    def tearDown(self) -> None:
        shutil.rmtree(self.tmp, ignore_errors=True)

    def manifest(self, **overrides: object) -> dict[str, object]:
        """The latest.json that tauri-action would write for this release's installers and signatures."""

        def entry(name: str) -> dict[str, str]:
            return {
                "signature": (self.assets / f"{name}.sig").read_text(encoding="utf-8"),
                "url": f"{release.RELEASE_DOWNLOAD_BASE}/v{self.VERSION}/{name}",
            }

        windows = entry(f"JobniacFast_{self.VERSION}_x64-setup.exe")
        linux = entry(f"JobniacFast_{self.VERSION}_amd64.AppImage")
        macos = entry(f"JobniacFast_{self.VERSION}_aarch64.app.tar.gz")
        data: dict[str, object] = {
            "version": self.VERSION,
            "notes": "n",
            "platforms": {
                "windows-x86_64": windows,
                "windows-x86_64-nsis": windows,
                "linux-x86_64": linux,
                "linux-x86_64-appimage": linux,
                "darwin-aarch64": macos,
                "darwin-aarch64-app": macos,
            },
        }
        data.update(overrides)
        return data

    def write_latest(self, **overrides: object) -> None:
        self.latest.write_text(json.dumps(self.manifest(**overrides)), encoding="utf-8")

    def release_payload(self, **overrides: object) -> dict[str, object]:
        assets = []
        local = [(name, self.assets / name) for name in release.expected_asset_names(self.VERSION)]
        local.append(("latest.json", self.latest))
        for index, (name, path) in enumerate(local):
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
            ("GET", "/releases/7"): (200, release_payload),
        }
        return release.GitHubApi("owner/repo", "token", FakeTransport(routes))

    def intent(self) -> release.ReleaseIntent:
        return release.tag_for_version(self.VERSION)

    def verify(self, payload: dict[str, object], tag_sha: str | None = None) -> None:
        release.verify_draft(self.api(payload, tag_sha), self.intent(), self.SHA, self.assets, self.latest, 7)

    def test_a_draft_with_a_placeholder_tag_name_is_read_by_id(self) -> None:
        self.verify(self.release_payload(tag_name="untagged-7bdbb8652c30764f6f8b"))

    def test_another_published_release_holding_the_tag_fails(self) -> None:
        other = self.release_payload(id=9, draft=False)
        routes: dict[tuple[str, str], tuple[int, object]] = {
            ("GET", "/git/ref/tags/"): (404, {}),
            ("GET", "/releases?"): (200, [self.release_payload(), other]),
            ("GET", "/releases/7"): (200, self.release_payload()),
        }
        api = release.GitHubApi("owner/repo", "token", FakeTransport(routes))
        with self.assertRaises(release.ReleaseError):
            release.verify_draft(api, self.intent(), self.SHA, self.assets, self.latest, 7)

    def test_matching_draft_passes(self) -> None:
        self.verify(self.release_payload())

    def test_wrong_commit_fails(self) -> None:
        with self.assertRaises(release.ReleaseError):
            self.verify(self.release_payload(), tag_sha="b" * 40)

    def test_published_or_wrong_flags_fail(self) -> None:
        for overrides in ({"draft": False}, {"prerelease": True}, {"target_commitish": "c" * 40}):
            with self.subTest(overrides=overrides), self.assertRaises(release.ReleaseError):
                self.verify(self.release_payload(**overrides))

    def test_asset_digest_or_set_mismatch_fails(self) -> None:
        payload = self.release_payload()
        payload["assets"][0]["digest"] = "sha256:" + "0" * 64  # type: ignore[index]
        with self.assertRaises(release.ReleaseError):
            self.verify(payload)
        payload = self.release_payload()
        payload["assets"].pop()  # type: ignore[attr-defined]
        with self.assertRaises(release.ReleaseError):
            self.verify(payload)

    def test_a_draft_without_latest_json_or_a_signature_fails(self) -> None:
        for dropped in (
            "latest.json",
            "JobniacFast_0.1.0_x64-setup.exe.sig",
            "JobniacFast_0.1.0_amd64.AppImage.sig",
            "JobniacFast_0.1.0_aarch64.app.tar.gz.sig",
        ):
            payload = self.release_payload()
            payload["assets"] = [a for a in payload["assets"] if a["name"] != dropped]  # type: ignore[index,union-attr]
            with self.subTest(dropped=dropped), self.assertRaises(release.ReleaseError):
                self.verify(payload)

    def test_a_latest_json_for_another_version_fails(self) -> None:
        self.write_latest(version="0.0.9")
        with self.assertRaises(release.ReleaseError):
            self.verify(self.release_payload())

    def test_a_latest_json_with_a_wrong_signature_url_or_platform_fails(self) -> None:
        good = self.manifest()
        platforms = good["platforms"]  # type: ignore[index]
        variants = []
        bad = json.loads(json.dumps(good))
        bad["platforms"]["linux-x86_64"]["signature"] = "other"
        variants.append(bad)
        bad = json.loads(json.dumps(good))
        bad["platforms"]["windows-x86_64"]["url"] = "https://example.org/Other_setup.exe"
        variants.append(bad)
        bad = json.loads(json.dumps(good))
        del bad["platforms"]["linux-x86_64"]
        variants.append(bad)
        bad = json.loads(json.dumps(good))
        del bad["platforms"]["darwin-aarch64"]
        variants.append(bad)
        bad = json.loads(json.dumps(good))
        bad["platforms"]["darwin-aarch64"]["signature"] = "other"
        variants.append(bad)
        self.assertIn("windows-x86_64", platforms)
        for variant in variants:
            self.latest.write_text(json.dumps(variant), encoding="utf-8")
            with self.assertRaises(release.ReleaseError):
                self.verify(self.release_payload())

    def test_a_latest_json_url_outside_this_repos_release_prefix_fails(self) -> None:
        bad = self.manifest()
        name = f"JobniacFast_{self.VERSION}_x64-setup.exe"
        bad["platforms"]["windows-x86_64"]["url"] = (  # type: ignore[index]
            f"https://github.com/other/repo/releases/download/v{self.VERSION}/{name}"
        )
        self.latest.write_text(json.dumps(bad), encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.verify(self.release_payload())

    def test_the_generated_manifest_passes_verification_and_has_the_static_format(self) -> None:
        for name in release.updater_installers(self.VERSION).values():
            (self.assets / name).write_bytes(b"x")
        built = release.build_updater_manifest(self.assets, self.VERSION, "notes", "2026-01-02T03:04:05Z")
        self.assertEqual(set(built), {"version", "notes", "pub_date", "platforms"})
        self.assertEqual(set(built["platforms"]), set(release.UPDATER_PLATFORMS))  # type: ignore[arg-type]
        self.latest.write_text(json.dumps(built), encoding="utf-8")
        self.verify(self.release_payload())

    def test_the_manifest_needs_the_installers_and_signatures(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.build_updater_manifest(self.tmp, self.VERSION, "n", "2026-01-02T03:04:05Z")

    def test_the_updater_manifest_command_writes_latest_json(self) -> None:
        assets = self.tmp / "version-assets"
        assets.mkdir()
        for name in release.updater_installers("0.4.2").values():
            (assets / name).write_bytes(b"x")
            (assets / f"{name}.sig").write_text("sig", encoding="utf-8")
        notes = self.tmp / "release-notes.md"
        notes.write_text("### Added\n\n- a thing\n", encoding="utf-8")
        out = self.tmp / "out" / "latest.json"
        with contextlib.redirect_stdout(io.StringIO()):
            status = release.main([
                "updater-manifest", "--version", "0.4.2", "--dir", str(assets), "--out", str(out),
                "--notes-file", str(notes), "--pub-date", "2026-01-02T03:04:05Z",
            ])
        self.assertEqual(status, 0)
        data = json.loads(out.read_text(encoding="utf-8"))
        self.assertEqual(data["version"], "0.4.2")
        self.assertEqual(data["notes"], "### Added\n\n- a thing")

    def test_a_missing_tag_is_fine_for_a_draft(self) -> None:
        routes: dict[tuple[str, str], tuple[int, object]] = {
            ("GET", "/git/ref/tags/"): (404, {}),
            ("GET", "/releases?"): (200, [self.release_payload()]),
            ("GET", "/releases/7"): (200, self.release_payload()),
        }
        api = release.GitHubApi("owner/repo", "token", FakeTransport(routes))
        release.verify_draft(api, self.intent(), self.SHA, self.assets, self.latest, 7)

    def publish_api(self, payload: dict[str, object], tag_sha: str | None) -> tuple[release.GitHubApi, FakeTransport]:
        published = dict(payload, draft=False)
        tag_route = (200, {"object": {"type": "commit", "sha": tag_sha}}) if tag_sha else (404, {})

        class PublishTransport(FakeTransport):
            def __call__(self, method: str, url: str, token: str, body: bytes | None, content_type: str) -> tuple[int, bytes]:
                result = super().__call__(method, url, token, body, content_type)
                if method == "PATCH":
                    # From here on the release reads as published.
                    self.routes[("GET", "/releases/7")] = (200, published)
                return result

        transport = PublishTransport({
            ("GET", "/git/ref/tags/"): tag_route,
            ("GET", "/releases?"): (200, [payload]),
            ("POST", "/git/refs"): (201, {}),
            ("PATCH", "/releases/7"): (200, published),
            ("GET", "/releases/7"): (200, payload),
        })
        return release.GitHubApi("owner/repo", "token", transport), transport

    def test_publish_tags_then_publishes_a_stable_release_as_latest(self) -> None:
        api, transport = self.publish_api(self.release_payload(), None)
        sent: list[bytes] = []
        original = api.transport

        def spy(method: str, url: str, token: str, body: bytes | None, content_type: str) -> tuple[int, bytes]:
            if body is not None:
                sent.append(body)
            # After the tag is created the ref exists at the merge commit.
            if method == "GET" and "/git/ref/tags/" in url and any(c[0] == "POST" for c in transport.calls):
                return 200, json.dumps({"object": {"type": "commit", "sha": self.SHA}}).encode("utf-8")
            return original(method, url, token, body, content_type)

        api.transport = spy
        message = release.publish_release(api, self.intent(), self.SHA, 7)
        self.assertIn("latest release", message)
        self.assertEqual(
            [call for call in transport.calls if call[0] in ("POST", "PATCH")],
            [("POST", "/git/refs"), ("PATCH", "/releases/7")],
        )
        tag_body, patch_body = (json.loads(b) for b in sent)
        self.assertEqual(tag_body, {"ref": "refs/tags/v0.1.0", "sha": self.SHA})
        self.assertEqual(patch_body, {"tag_name": "v0.1.0", "draft": False, "prerelease": False, "make_latest": "true"})

    def test_publish_of_a_dev_version_is_a_prerelease_and_never_latest(self) -> None:
        intent = release.tag_for_version("0.1.0-dev.2")
        payload = self.release_payload(prerelease=True, tag_name="v0.1.0-dev.2")
        api, _ = self.publish_api(payload, self.SHA)
        sent: list[bytes] = []
        original = api.transport
        api.transport = lambda m, u, t, b, c: (sent.append(b) if b else None, original(m, u, t, b, c))[1]
        self.assertIn("prerelease", release.publish_release(api, intent, self.SHA, 7))
        self.assertEqual(
            json.loads(sent[-1]),
            {"tag_name": "v0.1.0-dev.2", "draft": False, "prerelease": True, "make_latest": "false"},
        )

    def test_publish_fails_closed_when_the_tag_is_on_another_commit(self) -> None:
        api, transport = self.publish_api(self.release_payload(), "b" * 40)
        with self.assertRaises(release.ReleaseError):
            release.publish_release(api, self.intent(), self.SHA, 7)
        self.assertFalse([c for c in transport.calls if c[0] in ("POST", "PATCH")])

    def test_publish_reuses_a_tag_already_on_the_merge_commit(self) -> None:
        api, transport = self.publish_api(self.release_payload(), self.SHA)
        release.publish_release(api, self.intent(), self.SHA, 7)
        self.assertNotIn(("POST", "/git/refs"), transport.calls)
        self.assertIn(("PATCH", "/releases/7"), transport.calls)

    def test_publish_of_an_already_published_release_is_a_success_without_writes(self) -> None:
        api, transport = self.publish_api(self.release_payload(draft=False), self.SHA)
        self.assertIn("already published", release.publish_release(api, self.intent(), self.SHA, 7))
        self.assertFalse([c for c in transport.calls if c[0] in ("POST", "PATCH")])

    def test_publish_refuses_a_missing_release_a_wrong_target_or_another_published_holder(self) -> None:
        other = self.release_payload(id=9, draft=False)
        cases = {
            "missing": (None, []),
            "wrong target": (self.release_payload(target_commitish="c" * 40), []),
            "another published holder": (self.release_payload(), [other]),
        }
        for name, (payload, listing) in cases.items():
            routes: dict[tuple[str, str], tuple[int, object]] = {
                ("GET", "/git/ref/tags/"): (404, {}),
                ("GET", "/releases?"): (200, [*listing, *([payload] if payload else [])]),
            }
            if payload:
                routes[("GET", "/releases/7")] = (200, payload)
            transport = FakeTransport(routes)
            with self.subTest(case=name), self.assertRaises(release.ReleaseError):
                release.publish_release(release.GitHubApi("o/r", "t", transport), self.intent(), self.SHA, 7)
            self.assertFalse([c for c in transport.calls if c[0] in ("POST", "PATCH")])

    def test_publish_of_a_published_release_with_another_tag_name_is_refused(self) -> None:
        api, _ = self.publish_api(self.release_payload(draft=False, tag_name="untagged-abc"), self.SHA)
        with self.assertRaises(release.ReleaseError):
            release.publish_release(api, self.intent(), self.SHA, 7)

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

    def test_a_rerun_prunes_installers_signatures_and_latest_json(self) -> None:
        names = [
            "a-setup.exe", "a-setup.exe.sig", "a.AppImage.sig", "latest.json", "a.AppImage", "a.deb",
            "a_aarch64.dmg", "a_aarch64.app.tar.gz", "a_aarch64.app.tar.gz.sig",
        ]
        draft = {"id": 5, "tag_name": "v0.1.0", "draft": True, "prerelease": False, "target_commitish": self.SHA,
                 "assets": [{"id": i, "name": n} for i, n in enumerate(names)]}
        transport = FakeTransport({
            ("GET", "/releases?"): (200, [draft]),
            ("DELETE", "/releases/assets/"): (204, b""),
            ("PATCH", "/releases/5"): (200, {}),
        })
        api = release.GitHubApi("owner/repo", "token", transport)
        self.assertEqual(release.prepare_draft(api, release.tag_for_version("0.1.0"), self.SHA, "body"), 5)
        deleted = sorted(path for method, path in transport.calls if method == "DELETE")
        # setup.exe, its .sig, the AppImage .sig, latest.json, the DMG, the app archive and its .sig.
        self.assertEqual(
            deleted,
            [f"/releases/assets/{index}" for index in (0, 1, 2, 3, 6, 7, 8)],
        )

    def test_a_draft_from_another_commit_loses_every_asset(self) -> None:
        names = ["SHA256SUMS", "a.deb", "a.cdx.json", "a-setup.exe", "latest.json"]
        draft = {"id": 5, "tag_name": "untagged-abc", "name": "JobniacFast 0.1.1", "draft": True,
                 "prerelease": False, "target_commitish": "b" * 40,
                 "assets": [{"id": i, "name": n} for i, n in enumerate(names)]}
        transport = FakeTransport({
            ("GET", "/releases?"): (200, [draft]),
            ("DELETE", "/releases/assets/"): (204, b""),
            ("PATCH", "/releases/5"): (200, {}),
        })
        api = release.GitHubApi("owner/repo", "token", transport)
        self.assertEqual(release.prepare_draft(api, release.tag_for_version("0.1.1"), self.SHA, "body"), 5)
        deleted = sorted(path for method, path in transport.calls if method == "DELETE")
        self.assertEqual(deleted, [f"/releases/assets/{index}" for index in range(5)])

    def test_a_draft_with_a_placeholder_tag_name_is_reused_by_name_and_retargeted(self) -> None:
        # GitHub lists a draft as `untagged-<hash>`, so only its name identifies it.
        draft = {"id": 5, "tag_name": "untagged-7bdbb8652c30764f6f8b", "name": "JobniacFast 0.1.1", "draft": True,
                 "prerelease": False, "assets": []}
        elsewhere = {"id": 6, "tag_name": "untagged-0123456789abcdef0123", "name": "JobniacFast 0.1.0", "draft": True,
                     "prerelease": False, "assets": []}
        transport = FakeTransport({
            ("GET", "/releases?"): (200, [elsewhere, draft]),
            ("PATCH", "/releases/5"): (200, {}),
        })
        sent: list[bytes] = []
        api = release.GitHubApi("owner/repo", "token", lambda m, u, t, b, c: (sent.append(b) if b else None, transport(m, u, t, b, c))[1])
        self.assertEqual(release.prepare_draft(api, release.tag_for_version("0.1.1"), self.SHA, "body"), 5)
        self.assertEqual(
            json.loads(sent[-1]), {"tag_name": "v0.1.1", "target_commitish": self.SHA, "body": "body"}
        )
        self.assertNotIn(("POST", "/releases"), transport.calls)

    def test_two_drafts_matching_by_tag_and_name_refuse(self) -> None:
        drafts = [
            {"id": 5, "tag_name": "v0.1.1", "name": "x", "draft": True, "prerelease": False, "assets": []},
            {"id": 6, "tag_name": "untagged-abc", "name": "JobniacFast 0.1.1", "draft": True, "prerelease": False, "assets": []},
        ]
        api = release.GitHubApi("owner/repo", "token", FakeTransport({("GET", "/releases?"): (200, drafts)}))
        with self.assertRaises(release.ReleaseError):
            release.prepare_draft(api, release.tag_for_version("0.1.1"), self.SHA, "body")

    def test_a_published_release_with_the_name_refuses(self) -> None:
        published = {"id": 1, "tag_name": "v0.1.1", "name": "JobniacFast 0.1.1", "draft": False, "prerelease": False, "assets": []}
        api = release.GitHubApi("owner/repo", "token", FakeTransport({("GET", "/releases?"): (200, [published])}))
        with self.assertRaises(release.ReleaseError):
            release.prepare_draft(api, release.tag_for_version("0.1.1"), self.SHA, "body")

    def test_new_draft_is_created_once(self) -> None:
        transport = FakeTransport({
            ("GET", "/releases?"): (200, []),
            ("POST", "/releases"): (201, {"id": 42}),
        })
        api = release.GitHubApi("owner/repo", "token", transport)
        self.assertEqual(release.prepare_draft(api, release.tag_for_version("0.1.0"), self.SHA, "body"), 42)
        self.assertEqual(transport.calls.count(("POST", "/releases")), 1)


class NextVersion(unittest.TestCase):
    def test_the_first_release_takes_the_version_of_the_files(self) -> None:
        self.assertEqual(release.next_version("0.1.0", [], []), ("0.1.0", ""))

    def test_the_patch_after_the_highest_stable_tag(self) -> None:
        self.assertEqual(release.next_version("0.1.0", ["v0.1.0"], []), ("0.1.1", "v0.1.0"))
        # Numeric order, not text order: 0.1.10 is above 0.1.9.
        self.assertEqual(release.next_version("0.1.0", ["v0.1.9", "v0.1.10", "v0.1.2"], []), ("0.1.11", "v0.1.10"))

    def test_a_higher_untagged_file_version_is_the_floor(self) -> None:
        self.assertEqual(release.next_version("0.2.0", ["v0.1.0", "v0.1.4"], []), ("0.2.0", "v0.1.4"))
        self.assertEqual(release.next_version("1.0.0", ["v0.9.3"], []), ("1.0.0", "v0.9.3"))
        self.assertEqual(release.next_version("0.1.5-dev.1", ["v0.1.4"], []), ("0.1.5-dev.1", "v0.1.4"))

    def test_a_file_version_that_is_already_tagged_or_lower_gives_the_next_patch(self) -> None:
        self.assertEqual(release.next_version("0.2.0", ["v0.1.4", "v0.2.0"], []), ("0.2.1", "v0.2.0"))
        self.assertEqual(release.next_version("0.1.0", ["v0.2.3"], []), ("0.2.4", "v0.2.3"))
        # A dev version below the highest stable tag is not a floor.
        self.assertEqual(release.next_version("0.2.0-dev.1", ["v0.2.0"], []), ("0.2.1", "v0.2.0"))

    def test_a_tag_on_the_merge_commit_keeps_its_version(self) -> None:
        self.assertEqual(release.next_version("0.1.0", ["v0.1.0", "v0.1.1"], ["v0.1.1"]), ("0.1.1", "v0.1.0"))
        self.assertEqual(release.next_version("0.1.0", ["v0.1.1", "v0.1.2"], ["v0.1.1", "v0.1.2"]), ("0.1.2", "v0.1.1"))
        self.assertEqual(release.next_version("0.1.0", ["v0.1.0"], ["v0.1.0"]), ("0.1.0", ""))

    def test_dev_and_other_tags_do_not_count_as_the_highest_stable_tag(self) -> None:
        tags = ["v0.1.0", "v0.3.0-dev.2", "latest", "v1", "v1.2", "release-9.9.9", "v0.2.0-rc.1", "vx.y.z"]
        self.assertEqual(release.next_version("0.1.0", tags, []), ("0.1.1", "v0.1.0"))
        # A tag that is not a release tag on the merge commit is ignored too.
        self.assertEqual(release.next_version("0.1.0", tags, ["latest"]), ("0.1.1", "v0.1.0"))

    def test_the_previous_tag_is_the_highest_stable_tag_below_the_result(self) -> None:
        self.assertEqual(release.next_version("0.1.0", ["v0.1.0", "v0.2.0"], ["v0.1.0"]), ("0.1.0", ""))
        self.assertEqual(release.next_version("0.1.0", ["v0.1.0", "v0.2.0", "v0.1.1-dev.3"], ["v0.1.1-dev.3"]),
                         ("0.1.1-dev.3", "v0.1.0"))

    def test_an_unreleasable_file_version_is_refused(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.next_version("1.0", [], [])


class GitTags(unittest.TestCase):
    def run_git(self, root: Path, *arguments: str) -> str:
        completed = release.subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@example.org", "-c", "commit.gpgsign=false",
             "-c", "tag.gpgsign=false", *arguments],
            cwd=root, capture_output=True, encoding="utf-8", check=True,
        )
        return completed.stdout.strip()

    def test_lists_the_release_tags_and_those_at_the_merge_commit(self) -> None:
        if shutil.which("git") is None:
            self.skipTest("git is not installed")
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, True)
        self.run_git(root, "init", "-q")
        self.run_git(root, "commit", "-q", "--allow-empty", "-m", "one")
        self.run_git(root, "tag", "v0.1.0")
        self.run_git(root, "tag", "not-a-release")
        self.run_git(root, "commit", "-q", "--allow-empty", "-m", "two")
        self.run_git(root, "tag", "-a", "-m", "annotated", "v0.1.1")
        merge_sha = self.run_git(root, "rev-parse", "HEAD")
        tags, at_merge = release.git_release_tags(root, merge_sha)
        self.assertEqual(sorted(tags), ["v0.1.0", "v0.1.1"])
        self.assertEqual(at_merge, ["v0.1.1"])
        self.assertEqual(release.next_version("0.1.0", tags, at_merge), ("0.1.1", "v0.1.0"))

    def test_the_newest_stable_tag_must_be_an_ancestor_of_the_merge_commit(self) -> None:
        if shutil.which("git") is None:
            self.skipTest("git is not installed")
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, True)
        self.run_git(root, "init", "-q")
        self.run_git(root, "commit", "-q", "--allow-empty", "-m", "one")
        old_sha = self.run_git(root, "rev-parse", "HEAD")
        self.run_git(root, "commit", "-q", "--allow-empty", "-m", "two")
        self.run_git(root, "tag", "v0.1.0")
        self.run_git(root, "commit", "-q", "--allow-empty", "-m", "three")
        new_sha = self.run_git(root, "rev-parse", "HEAD")
        release.check_newest_release_is_ancestor(root, new_sha, ["v0.1.0"], [])
        # An old merge is not a descendant of the newest release.
        with self.assertRaises(release.ReleaseError):
            release.check_newest_release_is_ancestor(root, old_sha, ["v0.1.0"], [])
        # Skipped: a release tag on the merge commit (rerun), and no stable tag at all.
        release.check_newest_release_is_ancestor(root, old_sha, ["v0.1.0"], ["v0.1.0"])
        release.check_newest_release_is_ancestor(root, old_sha, ["v0.2.0-dev.1"], [])
        release.check_newest_release_is_ancestor(root, old_sha, [], [])

    def test_a_bad_sha_or_a_missing_repository_fails(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.git_release_tags(Path(tempfile.gettempdir()), "abc")
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, True)
        if shutil.which("git") is not None:
            with self.assertRaises(release.ReleaseError):
                release.git_release_tags(root, "a" * 40)


class SetVersion(unittest.TestCase):
    LOCK = (
        "# This file is automatically @generated by Cargo.\nversion = 4\n\n"
        '[[package]]\nname = "jobniac-core"\nversion = "0.1.0"\ndependencies = [\n "serde",\n]\n\n'
        '[[package]]\nname = "jobniac-cli"\nversion = "0.1.0"\n\n'
        '[[package]]\nname = "serde"\nversion = "0.1.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n'
        'checksum = "abc"\n\n'
        '[[package]]\nname = "jobniac-mcp"\nversion = "0.1.0"\n'
    )

    def setUp(self) -> None:
        self.repo = FakeRepo()
        self.addCleanup(self.repo.cleanup)
        self.lock = self.repo.root / "Cargo.lock"
        self.lock.write_text(self.LOCK, encoding="utf-8")

    def test_stamps_the_three_files_and_only_the_workspace_members_of_the_lock(self) -> None:
        release.set_version(self.repo.root, "0.4.2")
        self.assertEqual(set(release.read_versions(self.repo.root).values()), {"0.4.2"})
        self.assertEqual(
            self.lock.read_text(encoding="utf-8"),
            self.LOCK.replace('name = "jobniac-core"\nversion = "0.1.0"', 'name = "jobniac-core"\nversion = "0.4.2"')
            .replace('name = "jobniac-cli"\nversion = "0.1.0"', 'name = "jobniac-cli"\nversion = "0.4.2"')
            .replace('name = "jobniac-mcp"\nversion = "0.1.0"', 'name = "jobniac-mcp"\nversion = "0.4.2"'),
        )

    def test_is_idempotent_and_leaves_the_changelog_alone(self) -> None:
        changelog = self.repo.changelog.read_bytes()
        release.set_version(self.repo.root, "0.4.2")
        names = ["Cargo.toml", "src-tauri/tauri.conf.json", "package.json", "Cargo.lock"]
        first = {name: (self.repo.root / name).read_bytes() for name in names}
        release.set_version(self.repo.root, "0.4.2")
        self.assertEqual(first, {name: (self.repo.root / name).read_bytes() for name in names})
        self.assertEqual(changelog, self.repo.changelog.read_bytes())

    def test_a_dev_version_is_accepted_and_other_forms_and_a_missing_lock_are_refused(self) -> None:
        release.set_version(self.repo.root, "0.5.0-dev.1")
        self.assertEqual(release.resolve_version(self.repo.root).tag, "v0.5.0-dev.1")
        with self.assertRaises(release.ReleaseError):
            release.set_version(self.repo.root, "0.5")
        self.lock.unlink()
        with self.assertRaises(release.ReleaseError):
            release.set_version(self.repo.root, "0.5.1")

    def test_the_command_line_stamps(self) -> None:
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(release.main(["set-version", "0.4.3", "--repo-root", str(self.repo.root)]), 0)
        self.assertEqual(release.resolve_version(self.repo.root).version, "0.4.3")

    def test_the_repository_lock_follows_the_stamp(self) -> None:
        # Every workspace member of the real Cargo.lock carries the workspace version, so the stamp reaches all.
        lock = (REPO_ROOT / "Cargo.lock").read_text(encoding="utf-8")
        stamped = release._lock_with_version(lock, "9.9.9")
        members = re.findall(r'^name = "(jobniac-[^"]+)"\nversion = "9\.9\.9"\n(?!source)', stamped, re.MULTILINE)
        self.assertTrue(members)
        self.assertNotIn("9.9.9", re.sub(r'(?ms)^\[\[package\]\]\nname = "jobniac-[^\n]*\nversion = "9\.9\.9"', "", stamped))


class ReleaseNotes(unittest.TestCase):
    SHA = "a" * 40

    def setUp(self) -> None:
        self.repo = FakeRepo()
        self.addCleanup(self.repo.cleanup)

    def api(self, transport: FakeTransport) -> release.GitHubApi:
        return release.GitHubApi("owner/repo", "token", transport)

    def test_the_changelog_section_wins_without_an_api_call(self) -> None:
        transport = FakeTransport({})
        notes = release.release_notes(
            self.api(transport), self.repo.changelog, release.tag_for_version("0.1.0"), self.SHA, "v0.0.9"
        )
        self.assertEqual(notes, "- first\n")
        self.assertEqual(transport.calls, [])

    def test_a_version_without_a_section_uses_generated_notes(self) -> None:
        transport = FakeTransport({("POST", "/releases/generate-notes"): (200, {"name": "n", "body": "## What's Changed\n* x\n"})})
        sent: list[bytes] = []
        api = self.api(transport)
        original = api.transport
        api.transport = lambda m, u, t, b, c: (sent.append(b) if b else None, original(m, u, t, b, c))[1]
        notes = release.release_notes(api, self.repo.changelog, release.tag_for_version("0.1.1"), self.SHA, "v0.1.0")
        self.assertEqual(notes, "## What's Changed\n* x\n")
        self.assertEqual(
            json.loads(sent[0]),
            {"tag_name": "v0.1.1", "target_commitish": self.SHA, "previous_tag_name": "v0.1.0"},
        )

    def test_the_previous_tag_is_left_out_when_empty_and_checked_when_given(self) -> None:
        transport = FakeTransport({("POST", "/releases/generate-notes"): (200, {"body": "b"})})
        api = self.api(transport)
        sent: list[bytes] = []
        original = api.transport
        api.transport = lambda m, u, t, b, c: (sent.append(b) if b else None, original(m, u, t, b, c))[1]
        release.release_notes(api, self.repo.changelog, release.tag_for_version("0.1.1"), self.SHA, "")
        self.assertEqual(json.loads(sent[0]), {"tag_name": "v0.1.1", "target_commitish": self.SHA})
        with self.assertRaises(release.ReleaseError):
            release.release_notes(api, self.repo.changelog, release.tag_for_version("0.1.1"), self.SHA, "main")

    def test_an_empty_or_failed_generation_fails(self) -> None:
        for route in ((200, {"body": " "}), (200, {}), (500, {})):
            api = self.api(FakeTransport({("POST", "/releases/generate-notes"): route}))
            with self.subTest(route=route), self.assertRaises(release.ReleaseError):
                release.release_notes(api, self.repo.changelog, release.tag_for_version("0.1.1"), self.SHA, "")

    def test_an_empty_changelog_section_still_fails(self) -> None:
        self.repo.changelog.write_text("## [Unreleased]\n\n## [0.1.1] - 2026-01-02\n\n## [0.1.0] - 2026-01-01\n\n- a\n", encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            release.release_notes(self.api(FakeTransport({})), self.repo.changelog, release.tag_for_version("0.1.1"), self.SHA, "")


class ToolResolution(unittest.TestCase):
    def test_pnpm_is_resolved_with_which_so_windows_finds_pnpm_cmd(self) -> None:
        seen: list[list[str]] = []

        def fake_run(command: list[str], **_: object) -> object:
            seen.append(command)
            return type("Done", (), {"returncode": 0, "stdout": "{}"})()

        with mock.patch.object(release.shutil, "which", return_value="C:\\pnpm\\pnpm.cmd") as which, \
                mock.patch.object(release.subprocess, "run", fake_run):
            self.assertEqual(release.run_pnpm_licences(), {})
        which.assert_called_once_with("pnpm")
        self.assertEqual(seen[0][:2], ["C:\\pnpm\\pnpm.cmd", "licenses"])

    def test_a_missing_pnpm_fails_closed(self) -> None:
        with mock.patch.object(release.shutil, "which", return_value=None), \
                mock.patch.object(release.subprocess, "run", side_effect=AssertionError("must not run")):
            with self.assertRaises(release.ReleaseError):
                release.run_pnpm_licences()


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
    PDFIUM_MACOS = "../vendor/pdfium/mac-arm64/libpdfium.dylib"

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

    def test_default_macos_config_signs_ad_hoc_and_bundles_pdfium_as_a_framework(self) -> None:
        macos = self.load("tauri.macos.conf.json")["bundle"]
        # Code in Contents/Resources is not signed as code and breaks notarization, so the library goes
        # to Contents/Frameworks, where the bundler signs it with the app.
        self.assertNotIn("resources", macos)
        self.assertEqual(macos["macOS"]["frameworks"], [self.PDFIUM_MACOS])
        self.assertEqual(macos["macOS"]["signingIdentity"], "-")
        self.assertEqual(macos["targets"], ["app", "dmg"])
        self.assertEqual(macos["macOS"]["minimumSystemVersion"], "14.0")

    def test_the_library_validation_entitlement_is_ad_hoc_only(self) -> None:
        # The ad-hoc default carries it (no Team ID to match); the Developer ID build removes it.
        macos = self.load("tauri.macos.conf.json")["bundle"]["macOS"]
        self.assertEqual(macos["entitlements"], "macos/Entitlements.adhoc.plist")
        plist = (REPO_ROOT / "src-tauri" / macos["entitlements"]).read_text(encoding="utf-8")
        self.assertIn("com.apple.security.cs.disable-library-validation", plist)
        self.assertNotIn("allow-jit", plist)
        script = (RELEASE_DIR / "macos_build.sh").read_text(encoding="utf-8")
        self.assertIn('"entitlements":null', script.replace(" ", ""))

    def test_release_config_adds_sidecar_and_notices_for_macos_and_keeps_the_framework(self) -> None:
        effective = self.effective("tauri.macos.conf.json")
        bundle = effective["bundle"]
        self.assertEqual(bundle["externalBin"], self.STAGED_EXTERNAL_BIN)
        self.assertEqual(bundle["resources"], {self.STAGED_NOTICES: "THIRD-PARTY-NOTICES.txt"})
        self.assertEqual(bundle["macOS"]["frameworks"], [self.PDFIUM_MACOS])
        self.assertIs(bundle["createUpdaterArtifacts"], True)
        self.assertEqual(effective["plugins"]["updater"]["endpoints"], [release.UPDATER_ENDPOINT])

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

    def test_the_macos_leg_builds_through_the_signing_script_without_the_github_token(self) -> None:
        text = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        self.assertIn("platform: macos-arm64", text)
        self.assertIn("bundles: app,dmg", text)
        self.assertIn("library: libpdfium.dylib", text)
        self.assertIn("bash scripts/release/macos_build.sh", text)
        script = (RELEASE_DIR / "macos_build.sh").read_text(encoding="utf-8")
        self.assertIn("--config src-tauri/tauri.release.conf.json", script)
        self.assertNotIn("GITHUB_TOKEN", script)
        # The Apple secrets reach exactly one step, and never tauri-action (which holds the token).
        for name in (
            "APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "APPLE_SIGNING_IDENTITY",
            "APPLE_API_ISSUER", "APPLE_API_KEY", "APPLE_API_PRIVATE_KEY",
            "APPLE_ID", "APPLE_PASSWORD", "APPLE_TEAM_ID",
        ):
            with self.subTest(secret=name):
                self.assertEqual(text.count(f"{name}: ${{{{ secrets.{name} }}}}"), 1)
        tauri_action = text.split("uses: tauri-apps/tauri-action", 1)[1].split("- name:", 1)[0]
        self.assertNotIn("APPLE_", tauri_action)
        self.assertRegex(text, r"- name: Build and attach the draft assets[^\n]*\n\s+if: runner\.os != 'macOS'")

    def test_default_config_has_no_updater_so_dev_builds_need_no_key(self) -> None:
        default = self.load("tauri.conf.json")
        self.assertNotIn("updater", default.get("plugins", {}))
        self.assertIs(default["bundle"]["createUpdaterArtifacts"], False)

    def test_release_config_builds_signed_updater_artifacts(self) -> None:
        for platform_file in ("tauri.windows.conf.json", "tauri.linux.conf.json", "tauri.macos.conf.json"):
            with self.subTest(platform=platform_file):
                effective = self.effective(platform_file)
                self.assertIs(effective["bundle"]["createUpdaterArtifacts"], True)
                updater = effective["plugins"]["updater"]
                self.assertEqual(updater["endpoints"], [release.UPDATER_ENDPOINT])
                self.assertEqual(updater["windows"]["installMode"], "passive")
                self.assertTrue(updater["pubkey"])

    def test_release_workflow_signs_and_writes_the_updater_manifest(self) -> None:
        text = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        # latest.json and the signatures come from the `attach` job; two build legs would race to merge
        # latest.json. `includeUpdaterJson` is not a tauri-action v1 input and was silently ignored.
        self.assertIn("uploadUpdaterJson: false", text)
        self.assertIn("uploadUpdaterSignatures: false", text)
        self.assertNotIn("includeUpdaterJson", text)
        self.assertIn("release.py updater-manifest", text)
        for name in ("TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"):
            self.assertIn(f"{name}: ${{{{ secrets.{name} }}}}", text)

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

    def test_publish_is_the_last_job_with_its_own_write_permission(self) -> None:
        text = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        jobs = re.split(r"(?m)^  (?=[a-z][a-z-]*:\n)", text.split("\njobs:\n", 1)[1])
        names = [job.split(":", 1)[0] for job in jobs if job.strip()]
        self.assertEqual(names[-1], "publish")
        publish = next(job for job in jobs if job.startswith("publish:"))
        for gate in ("prepare", "draft", "build", "clean-machine", "clean-machine-macos", "attach", "attest", "verify-draft"):
            self.assertRegex(publish, rf"needs: \[[^\]]*\b{gate}\b")
        self.assertRegex(publish, r"permissions:\n\s+contents: write")
        self.assertIn("release.py publish", publish)
        self.assertEqual(text.count("release.py publish"), 1)
        self.assertRegex(publish, r"needs\.verify-draft\.result == 'success'")
        self.assertRegex(publish, r"needs\.attest\.result == 'skipped'")

    def job(self, name: str) -> str:
        text = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        jobs = re.split(r"(?m)^  (?=[a-z][a-z-]*:\n)", text.split("\njobs:\n", 1)[1])
        return next(job for job in jobs if job.startswith(f"{name}:"))

    def test_merged_runs_share_one_group_and_unmerged_closes_get_their_own(self) -> None:
        text = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
        self.assertIn(
            "  group: ${{ github.event.pull_request.merged == true && 'release' || format('skip-{0}', github.run_id) }}\n"
            "  cancel-in-progress: false",
            text,
        )

    def test_the_version_is_computed_from_the_tags_and_stamped_before_the_builds(self) -> None:
        prepare = self.job("prepare")
        self.assertRegex(prepare, r"fetch-depth: 0")
        self.assertIn('release.py next-version --merge-sha "$MERGE_SHA"', prepare)
        self.assertNotIn("resolve-version", (WORKFLOWS / "release.yml").read_text(encoding="utf-8"))
        self.assertIn("previous_tag: ${{ steps.resolve.outputs.previous_tag }}", prepare)
        build = self.job("build")
        stamp = build.index('release.py set-version "$VERSION"')
        self.assertLess(build.index("release.py check-checkout"), stamp)
        self.assertLess(stamp, build.index("pnpm install"))

    def test_draft_verify_and_publish_get_the_version_and_the_draft_id(self) -> None:
        self.assertRegex(self.job("draft"), r'--version "\$VERSION" --previous-tag "\$PREVIOUS_TAG"')
        self.assertRegex(self.job("draft"), r"needs: prepare")
        for name in ("verify-draft", "publish"):
            with self.subTest(job=name):
                job = self.job(name)
                self.assertIn('--version "$VERSION" --release-id "$RELEASE_ID"', job)
                self.assertIn("needs.draft.outputs.release_id", job)
        self.assertIn("gh api", self.job("attach"))
        self.assertIn("--notes-file release-notes.md", self.job("attach"))

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
