"""Tests for the platform-subset support of scripts/release/release.py.

Run from the repository root: `py -I -m unittest discover -s scripts/release/tests -p "test_*.py"`.
All data is synthetic (fake repository `example/app`, versions such as 1.2.3); no test touches the network.
"""

from __future__ import annotations

import contextlib
import io
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

WINDOWS, LINUX, MACOS = release.PLATFORMS
KEY = release.PLATFORM_UPDATER_KEYS
BASE = release.RELEASE_DOWNLOAD_BASE


def write_manifest(path: Path, version: str, keys: list[str], **extra: object) -> None:
    """A manifest of the given version whose entries point at that version's installers."""
    names = {
        "windows-x86_64": f"JobniacFast_{version}_x64-setup.exe",
        "linux-x86_64": f"JobniacFast_{version}_amd64.AppImage",
        "darwin-aarch64": f"JobniacFast_{version}_aarch64.app.tar.gz",
    }
    data: dict[str, object] = {
        "version": version,
        "notes": f"notes {version}",
        "pub_date": "2026-01-01T00:00:00Z",
        "platforms": {key: {"signature": f"sig-{version}-{key}", "url": f"{BASE}/v{version}/{names[key]}"} for key in keys},
    }
    data.update(extra)
    path.write_text(json.dumps(data), encoding="utf-8")


class TempDirCase(unittest.TestCase):
    VERSION = "1.2.3"
    PREVIOUS_VERSION = "1.2.2"

    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def make_assets(self, platforms: tuple[str, ...] = release.PLATFORMS) -> Path:
        """An assembled asset directory of the platforms: every file holds its own name, signatures a fixed text."""
        assets = self.tmp / "assets"
        assets.mkdir(exist_ok=True)
        for name in release.expected_asset_names(self.VERSION, platforms):
            content = f"signature of {name}" if name.endswith(".sig") else name
            (assets / name).write_bytes(content.encode("utf-8"))
        return assets

    def make_previous(self, per_platform: tuple[str, ...] = (), legacy: tuple[str, ...] = ()) -> Path:
        """A previous-manifests directory: per-platform files for some platforms and a latest.json for others."""
        previous = self.tmp / "previous"
        previous.mkdir(exist_ok=True)
        for platform in per_platform:
            write_manifest(previous / release.platform_manifest_name(KEY[platform]), self.PREVIOUS_VERSION, [KEY[platform]])
        if legacy:
            write_manifest(previous / "latest.json", self.PREVIOUS_VERSION, [KEY[platform] for platform in legacy])
        return previous

    def notes_file(self) -> Path:
        path = self.tmp / "notes.md"
        path.write_text("Release notes.\n", encoding="utf-8")
        return path

    def write_manifests(self, platforms: tuple[str, ...], previous: Path | None) -> tuple[Path, Path, list[str]]:
        assets = self.make_assets(platforms)
        out = self.tmp / "manifests"
        warnings = release.build_platform_manifests(assets, self.VERSION, platforms, previous, "n", "2026-02-02T00:00:00Z", out)
        return assets, out, warnings

    def names(self, directory: Path) -> list[str]:
        return sorted(entry.name for entry in directory.iterdir())


class PlatformSelection(unittest.TestCase):
    def test_a_valid_subset_is_returned_in_canonical_order(self) -> None:
        self.assertEqual(release.parse_platforms("macos-arm64,windows-x64"), (WINDOWS, MACOS))
        self.assertEqual(release.parse_platforms("linux-x64"), (LINUX,))
        self.assertEqual(release.parse_platforms(",".join(release.PLATFORMS)), release.PLATFORMS)

    def test_empty_unknown_and_duplicate_values_fail(self) -> None:
        for value in ("", "windows-x64,", "freebsd-x64", "windows-x64,windows-x64", "windows-x64, linux-x64"):
            with self.subTest(value=value), self.assertRaises(release.ReleaseError):
                release.parse_platforms(value)

    def test_the_default_asset_set_is_the_union_of_the_parts(self) -> None:
        expected = set(release.shared_asset_names("1.2.3"))
        for platform in release.PLATFORMS:
            expected |= set(release.platform_asset_names("1.2.3", platform))
        self.assertEqual(set(release.expected_asset_names("1.2.3")), expected)
        self.assertEqual(release.expected_asset_names("1.2.3"), sorted(release.expected_asset_names("1.2.3")))

    def test_a_subset_holds_its_platform_assets_and_the_shared_ones(self) -> None:
        names = set(release.expected_asset_names("1.2.3", (WINDOWS, MACOS)))
        self.assertEqual(
            names,
            set(release.shared_asset_names("1.2.3"))
            | set(release.platform_asset_names("1.2.3", WINDOWS))
            | set(release.platform_asset_names("1.2.3", MACOS)),
        )
        self.assertNotIn("JobniacFast_1.2.3_amd64.deb", names)
        self.assertIn("JobniacFast_1.2.3_x64-setup.exe.sig", names)
        self.assertIn("THIRD-PARTY-NOTICES.txt", names)
        self.assertIn("SHA256SUMS", names)
        self.assertEqual(len(release.sbom_asset_names("1.2.3")), 3)
        self.assertIn("JobniacFast-1.2.3-macos-arm64.payload-inventory.json", names)

    def test_the_exact_set_check_follows_the_platforms(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        try:
            for name in release.expected_asset_names("1.2.3", (WINDOWS,)):
                (tmp / name).write_bytes(b"x")
            self.assertEqual(
                len(release.check_exact_asset_set(tmp, "1.2.3", (WINDOWS,))),
                len(release.expected_asset_names("1.2.3", (WINDOWS,))),
            )
            with self.assertRaises(release.ReleaseError):
                release.check_exact_asset_set(tmp, "1.2.3")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


class SubsetAssembly(TempDirCase):
    def make_leg(self, name: str, files: list[str], notices: bytes = b"notices") -> Path:
        leg = self.tmp / "legs" / name
        leg.mkdir(parents=True)
        for file_name in files:
            (leg / file_name).write_bytes(notices if file_name == "THIRD-PARTY-NOTICES.txt" else file_name.encode("utf-8"))
        release.write_digest_manifest(leg, release.LEG_DIGESTS, set())
        return leg

    def leg_files(self, platform: str, with_sboms: bool = False) -> list[str]:
        files = [name for name in release.platform_asset_names(self.VERSION, platform)] + ["THIRD-PARTY-NOTICES.txt"]
        return files + (release.sbom_asset_names(self.VERSION) if with_sboms else [])

    def make_shared(self) -> Path:
        shared = self.tmp / "shared"
        shared.mkdir(exist_ok=True)
        for name in release.sbom_asset_names(self.VERSION):
            (shared / name).write_bytes(name.encode("utf-8"))
        return shared

    def test_all_three_legs_assemble_as_before(self) -> None:
        self.make_leg("windows-x64", self.leg_files(WINDOWS))
        self.make_leg("linux-x64", self.leg_files(LINUX, True))
        self.make_leg("macos-arm64", self.leg_files(MACOS))
        names = release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION)
        self.assertEqual(sorted(names + ["SHA256SUMS"]), release.expected_asset_names(self.VERSION))

    def test_a_subset_with_the_linux_leg_takes_the_sboms_from_it(self) -> None:
        self.make_leg("windows-x64", self.leg_files(WINDOWS))
        self.make_leg("linux-x64", self.leg_files(LINUX, True))
        release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION, (WINDOWS, LINUX))
        self.assertEqual(self.names(self.tmp / "out"), release.expected_asset_names(self.VERSION, (WINDOWS, LINUX)))

    def test_a_subset_without_linux_needs_the_shared_directory(self) -> None:
        self.make_leg("windows-x64", self.leg_files(WINDOWS))
        with self.assertRaisesRegex(release.ReleaseError, "--shared"):
            release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION, (WINDOWS,))

    def test_the_shared_directory_supplies_the_sboms(self) -> None:
        self.make_leg("windows-x64", self.leg_files(WINDOWS))
        self.make_leg("macos-arm64", self.leg_files(MACOS))
        shared = self.make_shared()
        release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION, (WINDOWS, MACOS), shared)
        out = self.tmp / "out"
        self.assertEqual(self.names(out), release.expected_asset_names(self.VERSION, (WINDOWS, MACOS)))
        sbom = release.sbom_asset_names(self.VERSION)[0]
        self.assertEqual((out / sbom).read_bytes(), (shared / sbom).read_bytes())
        release.verify_sha256sums(out)

    def test_the_shared_directory_also_works_with_the_linux_leg(self) -> None:
        self.make_leg("linux-x64", self.leg_files(LINUX))
        release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION, (LINUX,), self.make_shared())
        self.assertEqual(self.names(self.tmp / "out"), release.expected_asset_names(self.VERSION, (LINUX,)))

    def test_an_sbom_in_a_leg_and_in_the_shared_directory_fails(self) -> None:
        self.make_leg("linux-x64", self.leg_files(LINUX, True))
        with self.assertRaises(release.ReleaseError):
            release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION, (LINUX,), self.make_shared())

    def test_a_shared_directory_without_an_sbom_fails(self) -> None:
        self.make_leg("windows-x64", self.leg_files(WINDOWS))
        shared = self.make_shared()
        (shared / release.sbom_asset_names(self.VERSION)[2]).unlink()
        with self.assertRaises(release.ReleaseError):
            release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION, (WINDOWS,), shared)

    def test_a_leg_of_a_platform_outside_the_set_fails_the_exact_set(self) -> None:
        self.make_leg("windows-x64", self.leg_files(WINDOWS))
        self.make_leg("macos-arm64", self.leg_files(MACOS))
        with self.assertRaises(release.ReleaseError):
            release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION, (WINDOWS,), self.make_shared())

    def test_every_leg_must_carry_an_identical_notices_copy(self) -> None:
        self.make_leg("windows-x64", self.leg_files(WINDOWS), b"one")
        self.make_leg("macos-arm64", self.leg_files(MACOS), b"two")
        with self.assertRaisesRegex(release.ReleaseError, "disagree"):
            release.assemble(self.tmp / "legs", self.tmp / "out", self.VERSION, (WINDOWS, MACOS), self.make_shared())
        shutil.rmtree(self.tmp / "legs")
        self.make_leg("windows-x64", [name for name in self.leg_files(WINDOWS) if name != "THIRD-PARTY-NOTICES.txt"])
        with self.assertRaises(release.ReleaseError):
            release.assemble(self.tmp / "legs", self.tmp / "out2", self.VERSION, (WINDOWS,), self.make_shared())


class PlatformManifests(TempDirCase):
    def read(self, directory: Path, key: str) -> dict[str, object]:
        return json.loads((directory / release.platform_manifest_name(key)).read_text(encoding="utf-8"))

    def test_a_full_set_writes_three_files_and_the_combined_latest_json(self) -> None:
        assets, out, warnings = self.write_manifests(release.PLATFORMS, None)
        self.assertEqual(warnings, [])
        self.assertEqual(self.names(out), ["latest-darwin-aarch64.json", "latest-linux-x86_64.json", "latest-windows-x86_64.json", "latest.json"])
        combined = json.loads((out / "latest.json").read_text(encoding="utf-8"))
        self.assertEqual(set(combined["platforms"]), set(release.UPDATER_PLATFORMS))
        self.assertEqual(combined, release.build_updater_manifest(assets, self.VERSION, "n", "2026-02-02T00:00:00Z"))
        for key in release.UPDATER_PLATFORMS:
            data = self.read(out, key)
            self.assertEqual(set(data), {"version", "notes", "pub_date", "platforms"})
            self.assertEqual(data["platforms"], {key: combined["platforms"][key]})
            self.assertEqual(data["version"], self.VERSION)
        release.check_platform_manifests(out, self.VERSION, assets, release.PLATFORMS, None)

    def test_a_built_platform_gets_this_releases_installer_and_signature(self) -> None:
        _, out, _ = self.write_manifests((WINDOWS,), self.make_previous(per_platform=(LINUX, MACOS)))
        entry = self.read(out, "windows-x86_64")["platforms"]["windows-x86_64"]  # type: ignore[index]
        self.assertEqual(entry["url"], f"{BASE}/v1.2.3/JobniacFast_1.2.3_x64-setup.exe")
        self.assertEqual(entry["signature"], "signature of JobniacFast_1.2.3_x64-setup.exe.sig")

    def test_a_per_platform_previous_file_is_copied_byte_for_byte(self) -> None:
        previous = self.make_previous(per_platform=(LINUX, MACOS))
        # Unusual whitespace proves the bytes are copied, not re-serialised.
        raw = (previous / "latest-linux-x86_64.json").read_bytes().replace(b",", b" ,\r\n")
        (previous / "latest-linux-x86_64.json").write_bytes(raw)
        assets, out, warnings = self.write_manifests((WINDOWS,), previous)
        self.assertFalse(any("earlier build" in warning for warning in warnings))
        self.assertEqual((out / "latest-linux-x86_64.json").read_bytes(), raw)
        self.assertEqual((out / "latest-darwin-aarch64.json").read_bytes(), (previous / "latest-darwin-aarch64.json").read_bytes())
        release.check_platform_manifests(out, self.VERSION, assets, (WINDOWS,), previous)

    def test_a_missing_per_platform_file_is_derived_from_the_previous_latest_json(self) -> None:
        previous = self.make_previous(legacy=release.PLATFORMS)
        _, out, warnings = self.write_manifests((WINDOWS,), previous)
        self.assertEqual(warnings, [])
        legacy = json.loads((previous / "latest.json").read_text(encoding="utf-8"))
        derived = self.read(out, "linux-x86_64")
        self.assertEqual(derived["platforms"], {"linux-x86_64": legacy["platforms"]["linux-x86_64"]})
        self.assertEqual((derived["version"], derived["notes"], derived["pub_date"]), ("1.2.2", "notes 1.2.2", "2026-01-01T00:00:00Z"))

    def test_notes_and_pub_date_are_optional_in_the_previous_latest_json(self) -> None:
        previous = self.tmp / "previous"
        previous.mkdir()
        data = json.loads(self.make_previous(legacy=(LINUX,)).joinpath("latest.json").read_text(encoding="utf-8"))
        del data["notes"], data["pub_date"]
        (previous / "latest.json").write_text(json.dumps(data), encoding="utf-8")
        _, out, _ = self.write_manifests((WINDOWS, MACOS), previous)
        self.assertEqual(set(self.read(out, "linux-x86_64")), {"version", "platforms"})

    def test_the_per_platform_file_wins_over_the_previous_latest_json(self) -> None:
        previous = self.make_previous(per_platform=(LINUX,), legacy=release.PLATFORMS)
        write_manifest(previous / "latest-linux-x86_64.json", "1.2.1", ["linux-x86_64"])
        _, out, _ = self.write_manifests((WINDOWS,), previous)
        self.assertEqual(self.read(out, "linux-x86_64")["version"], "1.2.1")
        self.assertEqual(self.read(out, "darwin-aarch64")["version"], "1.2.2")

    def test_a_platform_with_no_earlier_build_is_skipped_with_a_warning(self) -> None:
        _, out, warnings = self.write_manifests((WINDOWS,), self.make_previous(per_platform=(LINUX,)))
        self.assertEqual(len(warnings), 2)
        self.assertTrue(any("macos-arm64" in warning for warning in warnings))
        self.assertEqual(self.names(out), ["latest-linux-x86_64.json", "latest-windows-x86_64.json"])

    def test_a_missing_or_none_previous_directory_carries_nothing(self) -> None:
        for previous in (None, self.tmp / "does-not-exist"):
            with self.subTest(previous=previous):
                shutil.rmtree(self.tmp / "manifests", ignore_errors=True)
                assets, out, warnings = self.write_manifests((WINDOWS,), previous)
                self.assertEqual(self.names(out), ["latest-windows-x86_64.json"])
                self.assertEqual(len(warnings), 3)
                release.check_platform_manifests(out, self.VERSION, assets, (WINDOWS,), previous)

    def test_a_partial_set_copies_the_previous_latest_json_unchanged(self) -> None:
        previous = self.make_previous(legacy=release.PLATFORMS)
        raw = (previous / "latest.json").read_bytes() + b"\n\n"
        (previous / "latest.json").write_bytes(raw)
        assets, out, _ = self.write_manifests((WINDOWS, LINUX), previous)
        self.assertEqual((out / "latest.json").read_bytes(), raw)
        release.check_platform_manifests(out, self.VERSION, assets, (WINDOWS, LINUX), previous)

    def test_a_partial_set_writes_no_latest_json_without_a_previous_one(self) -> None:
        _, out, warnings = self.write_manifests((WINDOWS,), self.make_previous(per_platform=(LINUX, MACOS)))
        self.assertFalse((out / "latest.json").exists())
        self.assertTrue(any("latest.json" in warning for warning in warnings))

    def test_a_full_set_ignores_the_previous_manifests(self) -> None:
        previous = self.make_previous(legacy=release.PLATFORMS)
        (previous / "latest.json").write_text("not json", encoding="utf-8")
        _, out, warnings = self.write_manifests(release.PLATFORMS, previous)
        self.assertEqual(warnings, [])
        self.assertEqual(self.read(out, "linux-x86_64")["version"], self.VERSION)

    def test_a_missing_installer_signature_fails(self) -> None:
        assets = self.make_assets((WINDOWS,))
        (assets / "JobniacFast_1.2.3_x64-setup.exe.sig").unlink()
        with self.assertRaises(release.ReleaseError):
            release.build_platform_manifests(assets, self.VERSION, (WINDOWS,), None, "n", "d", self.tmp / "out")

    def test_the_updater_manifest_of_a_subset_holds_only_its_keys(self) -> None:
        assets = self.make_assets((WINDOWS, LINUX))
        manifest = release.build_updater_manifest(assets, self.VERSION, "n", "d", (WINDOWS, LINUX))
        self.assertEqual(set(manifest["platforms"]), {"windows-x86_64", "linux-x86_64"})  # type: ignore[call-overload]
        path = self.tmp / "latest.json"
        release.write_json(path, manifest)
        release.check_updater_manifest(path, self.VERSION, assets, (WINDOWS, LINUX))
        with self.assertRaises(release.ReleaseError):
            release.check_updater_manifest(path, self.VERSION, assets)


class CarriedValidation(TempDirCase):
    def carry(self, previous: Path, version: str | None = None) -> None:
        assets = self.make_assets((WINDOWS,))
        release.build_platform_manifests(assets, version or self.VERSION, (WINDOWS,), previous, "n", "d", self.tmp / "out")

    def previous_with(self, name: str, text: str) -> Path:
        previous = self.tmp / "previous"
        previous.mkdir(exist_ok=True)
        (previous / name).write_text(text, encoding="utf-8")
        return previous

    def test_a_carried_version_must_be_strictly_lower(self) -> None:
        for previous_version, current in (("1.2.3", "1.2.3"), ("1.2.4", "1.2.3"), ("1.2.3", "1.2.3-dev.1"), ("1.2.3-dev.2", "1.2.3-dev.2")):
            with self.subTest(previous=previous_version, current=current):
                shutil.rmtree(self.tmp / "previous", ignore_errors=True)
                previous = self.tmp / "previous"
                previous.mkdir()
                write_manifest(previous / "latest-linux-x86_64.json", previous_version, ["linux-x86_64"])
                self.VERSION = current
                with self.assertRaises(release.ReleaseError):
                    self.carry(previous, current)

    def test_a_dev_version_below_the_stable_one_is_accepted(self) -> None:
        previous = self.tmp / "previous"
        previous.mkdir()
        write_manifest(previous / "latest-linux-x86_64.json", "1.2.3-dev.4", ["linux-x86_64"])
        self.carry(previous, "1.2.3")
        self.assertTrue((self.tmp / "out" / "latest-linux-x86_64.json").is_file())

    def test_malformed_previous_files_fail_with_a_fixed_message(self) -> None:
        good = {"signature": "s", "url": f"{BASE}/v1.2.2/JobniacFast_1.2.2_amd64.AppImage"}
        cases = {
            "latest-linux-x86_64.json": [
                "not json",
                "[]",
                '{"version": "1.2.2", "version": "1.2.1", "platforms": {}}',
                json.dumps({"version": "1.2.2", "platforms": {"linux-x86_64": good, "darwin-aarch64": good}}),
                json.dumps({"version": "1.2.2", "platforms": {"darwin-aarch64": good}}),
                json.dumps({"version": "banana", "platforms": {"linux-x86_64": good}}),
                json.dumps({"platforms": {"linux-x86_64": good}}),
                json.dumps({"version": "1.2.2", "platforms": {"linux-x86_64": dict(good, url="https://example.com/x")}}),
                json.dumps({"version": "1.2.2", "platforms": {"linux-x86_64": dict(good, url=f"{BASE}/v1.2.1/x")}}),
                json.dumps({"version": "1.2.2", "platforms": {"linux-x86_64": {"url": good["url"]}}}),
                json.dumps({"version": "1.2.2", "platforms": []}),
            ],
            "latest.json": [
                "not json",
                json.dumps({"version": "1.2.2", "platforms": {"windows-x86_64": good}}),
                json.dumps({"version": "1.2.2", "platforms": {"linux-x86_64": dict(good, url="https://example.com/x")}}),
            ],
        }
        for name, texts in cases.items():
            for text in texts:
                with self.subTest(name=name, text=text[:50]):
                    shutil.rmtree(self.tmp / "previous", ignore_errors=True)
                    previous = self.previous_with(name, text)
                    with self.assertRaisesRegex(release.ReleaseError, "previous updater manifest .* rejected"):
                        self.carry(previous)

    def test_a_partial_set_validates_the_legacy_copy_for_every_platform(self) -> None:
        previous = self.previous_with(
            "latest.json",
            json.dumps({"version": "1.2.2", "platforms": {"windows-x86_64": {"signature": "s", "url": "https://example.com/x"}}}),
        )
        write_manifest(previous / "latest-linux-x86_64.json", "1.2.2", ["linux-x86_64"])
        write_manifest(previous / "latest-darwin-aarch64.json", "1.2.2", ["darwin-aarch64"])
        with self.assertRaises(release.ReleaseError):
            self.carry(previous)


class ManifestCheck(TempDirCase):
    def setUp(self) -> None:
        super().setUp()
        self.platforms = (WINDOWS,)
        self.previous = self.make_previous(per_platform=(LINUX,), legacy=(LINUX, MACOS))
        self.assets, self.out, _ = self.write_manifests(self.platforms, self.previous)

    def check(self) -> None:
        release.check_platform_manifests(self.out, self.VERSION, self.assets, self.platforms, self.previous)

    def test_the_written_manifests_pass(self) -> None:
        self.check()

    def test_a_carried_file_that_differs_from_the_previous_one_fails(self) -> None:
        path = self.out / "latest-linux-x86_64.json"
        path.write_bytes(path.read_bytes() + b" ")
        with self.assertRaises(release.ReleaseError):
            self.check()

    def test_a_derived_file_must_equal_the_derivation(self) -> None:
        path = self.out / "latest-darwin-aarch64.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        data["version"] = "9.9.9"
        path.write_text(json.dumps(data), encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.check()

    def test_a_derived_file_that_is_json_equal_but_reformatted_passes(self) -> None:
        path = self.out / "latest-darwin-aarch64.json"
        path.write_text(json.dumps(json.loads(path.read_text(encoding="utf-8")), indent=4), encoding="utf-8")
        self.check()

    def test_a_new_file_must_name_this_releases_installer_and_signature(self) -> None:
        path = self.out / "latest-windows-x86_64.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        data["platforms"]["windows-x86_64"]["signature"] = "forged"
        path.write_text(json.dumps(data), encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.check()

    def test_a_new_file_with_another_key_fails(self) -> None:
        path = self.out / "latest-windows-x86_64.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        data["platforms"]["linux-x86_64"] = data["platforms"]["windows-x86_64"]
        path.write_text(json.dumps(data), encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.check()

    def test_a_missing_file_fails(self) -> None:
        for name in ("latest-windows-x86_64.json", "latest-linux-x86_64.json", "latest-darwin-aarch64.json", "latest.json"):
            with self.subTest(name=name):
                shutil.rmtree(self.out)
                self.write_manifests(self.platforms, self.previous)
                (self.out / name).unlink()
                with self.assertRaises(release.ReleaseError):
                    self.check()

    def test_an_unexpected_file_fails(self) -> None:
        (self.out / "extra.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.check()

    def test_a_legacy_latest_json_that_differs_fails(self) -> None:
        (self.out / "latest.json").write_bytes(b"{}")
        with self.assertRaises(release.ReleaseError):
            self.check()

    def test_a_legacy_latest_json_without_a_previous_one_must_be_absent(self) -> None:
        shutil.rmtree(self.out)
        previous = self.make_previous(per_platform=(LINUX, MACOS))
        (previous / "latest.json").unlink(missing_ok=True)
        assets, out, _ = self.write_manifests(self.platforms, previous)
        (out / "latest.json").write_text("{}", encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            release.check_platform_manifests(out, self.VERSION, assets, self.platforms, previous)

    def test_a_full_set_needs_a_valid_combined_latest_json(self) -> None:
        shutil.rmtree(self.out)
        assets, out, _ = self.write_manifests(release.PLATFORMS, None)
        release.check_platform_manifests(out, self.VERSION, assets, release.PLATFORMS, None)
        data = json.loads((out / "latest.json").read_text(encoding="utf-8"))
        del data["platforms"]["linux-x86_64"]
        (out / "latest.json").write_text(json.dumps(data), encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            release.check_platform_manifests(out, self.VERSION, assets, release.PLATFORMS, None)


class AssetTransport:
    """A stub GitHub transport that serves a latest release and its asset bodies, and records the calls."""

    def __init__(self, release_status: int, release_payload: object, bodies: dict[int, bytes] | None = None) -> None:
        self.release_status = release_status
        self.release_payload = release_payload
        self.bodies = bodies or {}
        self.calls: list[tuple[str, str, str]] = []

    def __call__(self, method: str, url: str, token: str, body: bytes | None, content_type: str) -> tuple[int, bytes]:
        path = "/" + url.split("/repos/", 1)[1].split("/", 2)[2]
        self.calls.append((method, path, content_type))
        if path == "/releases/latest":
            return self.release_status, json.dumps(self.release_payload).encode("utf-8")
        if path.startswith("/releases/assets/"):
            asset_id = int(path.rsplit("/", 1)[1])
            if asset_id in self.bodies:
                return 200, self.bodies[asset_id]
        return 404, b'{"message": "Not Found"}'


class FetchPreviousManifests(TempDirCase):
    def api(self, transport: AssetTransport) -> release.GitHubApi:
        return release.GitHubApi("example/app", "token", transport)

    def asset(self, asset_id: int, name: str, size: int = 10) -> dict[str, object]:
        return {"id": asset_id, "name": name, "size": size}

    def test_only_the_manifests_are_downloaded(self) -> None:
        assets = [
            self.asset(1, "latest.json"),
            self.asset(2, "latest-linux-x86_64.json"),
            self.asset(3, "latest-windows-x86_64.json"),
            self.asset(4, "JobniacFast_1.2.2_amd64.AppImage", 10**9),
            self.asset(5, "latest-freebsd-x86_64.json"),
            self.asset(6, "latest.json.sig"),
            self.asset(7, "SHA256SUMS"),
        ]
        transport = AssetTransport(200, {"assets": assets}, {1: b"legacy", 2: b"linux", 3: b"windows"})
        out = self.tmp / "previous"
        names = release.fetch_previous_manifests(self.api(transport), out)
        self.assertEqual(names, ["latest-linux-x86_64.json", "latest-windows-x86_64.json", "latest.json"])
        self.assertEqual(self.names(out), names)
        self.assertEqual((out / "latest-linux-x86_64.json").read_bytes(), b"linux")
        downloads = [call for call in transport.calls if call[1].startswith("/releases/assets/")]
        self.assertEqual({call[1] for call in downloads}, {"/releases/assets/1", "/releases/assets/2", "/releases/assets/3"})
        self.assertTrue(all(call[0] == "GET" and call[2] == "application/octet-stream" for call in downloads))

    def test_no_published_release_gives_an_empty_directory(self) -> None:
        transport = AssetTransport(404, {"message": "Not Found"})
        out = self.tmp / "nested" / "previous"
        self.assertEqual(release.fetch_previous_manifests(self.api(transport), out), [])
        self.assertTrue(out.is_dir())
        self.assertEqual(self.names(out), [])
        self.assertEqual(transport.calls, [("GET", "/releases/latest", "application/json")])

    def test_a_release_without_manifests_gives_an_empty_directory(self) -> None:
        transport = AssetTransport(200, {"assets": [self.asset(4, "SHA256SUMS")]})
        self.assertEqual(release.fetch_previous_manifests(self.api(transport), self.tmp / "previous"), [])

    def test_a_manifest_declared_over_one_mebibyte_is_rejected(self) -> None:
        transport = AssetTransport(200, {"assets": [self.asset(1, "latest.json", release.API_MAX_BYTES + 1)]}, {1: b"x"})
        with self.assertRaises(release.ReleaseError):
            release.fetch_previous_manifests(self.api(transport), self.tmp / "previous")
        self.assertEqual([call for call in transport.calls if call[1].startswith("/releases/assets/")], [])

    def test_a_download_over_one_mebibyte_is_rejected(self) -> None:
        transport = AssetTransport(200, {"assets": [self.asset(1, "latest.json", 10)]}, {1: b"x" * (release.API_MAX_BYTES + 1)})
        with self.assertRaises(release.ReleaseError):
            release.fetch_previous_manifests(self.api(transport), self.tmp / "previous")
        self.assertFalse((self.tmp / "previous" / "latest.json").exists())

    def test_a_download_of_exactly_one_mebibyte_is_accepted(self) -> None:
        body = b"x" * release.API_MAX_BYTES
        transport = AssetTransport(200, {"assets": [self.asset(1, "latest.json", len(body))]}, {1: body})
        self.assertEqual(release.fetch_previous_manifests(self.api(transport), self.tmp / "previous"), ["latest.json"])

    def test_other_failures_are_rejected(self) -> None:
        cases = [
            AssetTransport(500, {}),
            AssetTransport(200, {"assets": "nope"}),
            AssetTransport(200, {"assets": [self.asset(1, "latest.json"), self.asset(2, "latest.json")]}, {1: b"a", 2: b"b"}),
            AssetTransport(200, {"assets": [self.asset(1, "latest.json")]}, {}),
            AssetTransport(200, {"assets": [{"name": "latest.json", "size": 3}]}),
        ]
        for transport in cases:
            with self.subTest(transport=transport.release_payload), self.assertRaises(release.ReleaseError):
                release.fetch_previous_manifests(self.api(transport), self.tmp / "previous")

    def test_the_default_transport_asks_for_the_asset_body_only_on_a_download(self) -> None:
        captured: list[dict[str, str]] = []

        class Response:
            status = 200

            def __enter__(self) -> "Response":
                return self

            def __exit__(self, *_: object) -> None:
                return None

            def read(self, _: int) -> bytes:
                return b"{}"

        class Opener:
            def open(self, request: object, timeout: int) -> Response:
                captured.append({key.lower(): value for key, value in request.header_items()})  # type: ignore[attr-defined]
                return Response()

        original = release.urllib.request.build_opener
        release.urllib.request.build_opener = lambda *_: Opener()  # type: ignore[assignment]
        try:
            release.default_transport("GET", "https://api.github.com/repos/example/app/releases/assets/1", "t", None, "application/octet-stream")
            release.default_transport("GET", "https://api.github.com/repos/example/app/releases/latest", "t", None, "application/json")
        finally:
            release.urllib.request.build_opener = original  # type: ignore[assignment]
        self.assertEqual(captured[0]["accept"], "application/octet-stream")
        self.assertEqual(captured[1]["accept"], "application/vnd.github+json")


class DraftWithManifestsDirectory(TempDirCase):
    SHA = "a" * 40

    def setUp(self) -> None:
        super().setUp()
        self.platforms = (WINDOWS, MACOS)
        self.previous = self.make_previous(per_platform=(LINUX,))
        self.assets, self.manifests, _ = self.write_manifests(self.platforms, self.previous)

    def payload(self, manifests_dir: Path | None = None, **overrides: object) -> dict[str, object]:
        files = [(entry.name, entry) for entry in self.assets.iterdir()]
        files += [(entry.name, entry) for entry in (manifests_dir or self.manifests).iterdir()]
        payload: dict[str, object] = {
            "id": 7,
            "tag_name": "v1.2.3",
            "draft": True,
            "prerelease": False,
            "target_commitish": self.SHA,
            "assets": [
                {"id": index, "name": name, "size": path.stat().st_size, "digest": f"sha256:{release.sha256_file(path)}"}
                for index, (name, path) in enumerate(files)
            ],
        }
        payload.update(overrides)
        return payload

    def verify(self, payload: dict[str, object], manifests: Path | None = None, platforms: tuple[str, ...] | None = None) -> None:
        routes = {
            ("GET", "/git/ref/tags/"): (200, {"object": {"type": "commit", "sha": self.SHA}}),
            ("GET", "/releases?"): (200, [payload]),
            ("GET", "/releases/7"): (200, payload),
        }
        from test_release import FakeTransport

        api = release.GitHubApi("example/app", "token", FakeTransport(routes))
        release.verify_draft(api, release.tag_for_version(self.VERSION), self.SHA, self.assets, None, 7,
                             platforms or self.platforms, manifests or self.manifests)

    def test_a_draft_with_the_assets_and_every_manifest_passes(self) -> None:
        self.verify(self.payload())

    def test_a_draft_missing_a_carried_manifest_fails(self) -> None:
        payload = self.payload()
        payload["assets"] = [asset for asset in payload["assets"] if asset["name"] != "latest-linux-x86_64.json"]  # type: ignore[attr-defined,index]
        with self.assertRaises(release.ReleaseError):
            self.verify(payload)

    def test_a_draft_with_an_extra_asset_fails(self) -> None:
        payload = self.payload()
        payload["assets"].append({"id": 99, "name": "extra.bin", "size": 1, "digest": "sha256:" + "0" * 64})  # type: ignore[attr-defined]
        with self.assertRaises(release.ReleaseError):
            self.verify(payload)

    def test_a_manifest_with_a_different_digest_fails(self) -> None:
        payload = self.payload()
        for asset in payload["assets"]:  # type: ignore[attr-defined]
            if asset["name"] == "latest-linux-x86_64.json":
                asset["digest"] = "sha256:" + "0" * 64
        with self.assertRaises(release.ReleaseError):
            self.verify(payload)

    def test_the_local_assets_must_match_the_platforms(self) -> None:
        with self.assertRaises(release.ReleaseError):
            self.verify(self.payload(), platforms=release.PLATFORMS)

    def test_a_manifests_directory_without_a_new_platforms_file_fails(self) -> None:
        (self.manifests / "latest-windows-x86_64.json").unlink()
        with self.assertRaises(release.ReleaseError):
            self.verify(self.payload())

    def test_a_new_manifest_that_does_not_match_the_installer_fails(self) -> None:
        path = self.manifests / "latest-windows-x86_64.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        data["platforms"]["windows-x86_64"]["signature"] = "forged"
        path.write_text(json.dumps(data), encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.verify(self.payload())

    def test_a_foreign_file_in_the_manifests_directory_fails(self) -> None:
        (self.manifests / "notes.txt").write_text("x", encoding="utf-8")
        with self.assertRaises(release.ReleaseError):
            self.verify(self.payload())

    def test_the_other_draft_checks_still_apply(self) -> None:
        for overrides in ({"draft": False}, {"prerelease": True}, {"target_commitish": "b" * 40}):
            with self.subTest(overrides=overrides), self.assertRaises(release.ReleaseError):
                self.verify(self.payload(**overrides))

    def test_the_latest_json_path_still_works_with_a_platform_subset(self) -> None:
        from test_release import FakeTransport

        latest = self.tmp / "latest.json"
        release.write_json(latest, release.build_updater_manifest(self.assets, self.VERSION, "n", "d", self.platforms))
        payload = self.payload()
        payload["assets"] = [a for a in payload["assets"] if not a["name"].startswith("latest")]  # type: ignore[attr-defined,index]
        payload["assets"].append({"id": 50, "name": "latest.json", "size": latest.stat().st_size,  # type: ignore[attr-defined]
                                  "digest": f"sha256:{release.sha256_file(latest)}"})
        routes = {
            ("GET", "/git/ref/tags/"): (200, {"object": {"type": "commit", "sha": self.SHA}}),
            ("GET", "/releases?"): (200, [payload]),
            ("GET", "/releases/7"): (200, payload),
        }
        api = release.GitHubApi("example/app", "token", FakeTransport(routes))
        release.verify_draft(api, release.tag_for_version(self.VERSION), self.SHA, self.assets, latest, 7, self.platforms)


class PlatformNotes(TempDirCase):
    def notes(self, platforms: tuple[str, ...], previous: Path | None) -> str:
        return release.platform_notes(self.VERSION, platforms, previous, "example/app")

    def test_a_full_set_prints_nothing(self) -> None:
        self.assertEqual(self.notes(release.PLATFORMS, self.make_previous(legacy=release.PLATFORMS)), "")

    def test_a_partial_set_prints_a_marked_table(self) -> None:
        text = self.notes((WINDOWS,), self.make_previous(per_platform=(LINUX,), legacy=(MACOS,)))
        lines = text.splitlines()
        self.assertEqual(lines[0], "<!-- platforms -->")
        self.assertEqual(lines[-1], "<!-- /platforms -->")
        self.assertTrue(text.endswith("\n"))
        self.assertIn("| Windows (x64) | 1.2.3 |", lines)
        self.assertIn("| Linux (x64) | [1.2.2](https://github.com/example/app/releases/tag/v1.2.2) |", lines)
        self.assertIn("| macOS (Apple Silicon) | [1.2.2](https://github.com/example/app/releases/tag/v1.2.2) |", lines)
        self.assertEqual(sum(1 for line in lines if line.startswith("| ") and "---" not in line), 4)

    def test_a_platform_with_nothing_carried_is_not_available_yet(self) -> None:
        text = self.notes((WINDOWS,), None)
        self.assertIn("| Linux (x64) | not available yet |", text)
        self.assertIn("| macOS (Apple Silicon) | not available yet |", text)

    def test_a_carried_version_that_is_not_lower_fails(self) -> None:
        previous = self.make_previous(per_platform=(LINUX,))
        write_manifest(previous / "latest-linux-x86_64.json", "1.2.3", ["linux-x86_64"])
        with self.assertRaises(release.ReleaseError):
            self.notes((WINDOWS,), previous)

    def test_an_invalid_repository_is_rejected(self) -> None:
        with self.assertRaises(release.ReleaseError):
            release.platform_notes(self.VERSION, (WINDOWS,), None, "example/app) [x](https://evil")


class BodyAnnotation(TempDirCase):
    def section(self, platforms: tuple[str, ...] = (WINDOWS,)) -> str:
        return release.platform_notes(self.VERSION, platforms, None, "example/app")

    def test_a_section_is_appended_to_a_body_without_one(self) -> None:
        body = "## Changes\n\n- one\n"
        updated = release.apply_platform_section(body, self.section())
        self.assertTrue(updated.startswith(body))
        self.assertEqual(updated.count("<!-- platforms -->"), 1)
        self.assertTrue(updated.endswith("<!-- /platforms -->\n"))

    def test_a_section_is_appended_to_an_empty_body(self) -> None:
        self.assertEqual(release.apply_platform_section("", self.section()), self.section())

    def test_a_second_run_does_not_duplicate_the_section(self) -> None:
        once = release.apply_platform_section("Notes.\n", self.section())
        self.assertEqual(release.apply_platform_section(once, self.section()), once)

    def test_an_existing_section_is_replaced_in_place(self) -> None:
        once = release.apply_platform_section("Before.\n", self.section((WINDOWS,)))
        body = once + "\nAfter.\n"
        updated = release.apply_platform_section(body, self.section((WINDOWS, LINUX)))
        self.assertEqual(updated.count("<!-- platforms -->"), 1)
        self.assertIn("| Linux (x64) | 1.2.3 |", updated)
        self.assertNotIn("not available yet | \n", updated.split("After.")[0].split("| Linux")[0])
        self.assertTrue(updated.startswith("Before.\n"))
        self.assertTrue(updated.endswith("\nAfter.\n"))

    def test_an_empty_section_removes_a_stale_one(self) -> None:
        body = release.apply_platform_section("Before.\n", self.section())
        self.assertEqual(release.apply_platform_section(body, ""), "Before.\n")
        self.assertEqual(release.apply_platform_section("Before.\n", ""), "Before.\n")

    def test_annotate_draft_patches_only_a_changed_body(self) -> None:
        state = {"body": "Notes.\n", "patches": 0}

        def transport(method: str, url: str, token: str, body: bytes | None, content_type: str) -> tuple[int, bytes]:
            if method == "PATCH":
                state["patches"] += 1
                state["body"] = json.loads(body.decode("utf-8"))["body"]  # type: ignore[union-attr]
            return 200, json.dumps({"id": 7, "body": state["body"]}).encode("utf-8")

        api = release.GitHubApi("example/app", "token", transport)
        self.assertTrue(release.annotate_draft(api, 7, self.section()))
        self.assertFalse(release.annotate_draft(api, 7, self.section()))
        self.assertEqual(state["patches"], 1)
        self.assertEqual(state["body"].count("<!-- platforms -->"), 1)  # type: ignore[attr-defined]


class DraftReuse(unittest.TestCase):
    def test_platform_manifests_are_recognised_by_name(self) -> None:
        for key in release.UPDATER_PLATFORMS:
            self.assertTrue(release._is_platform_manifest_name(f"latest-{key}.json"))
        self.assertFalse(release._is_platform_manifest_name("latest.json"))
        self.assertFalse(release._is_platform_manifest_name("latest-other.json"))

    def test_the_assembled_asset_set_never_holds_a_manifest(self) -> None:
        tmp = Path(tempfile.mkdtemp())
        try:
            for name in release.expected_asset_names("1.2.3"):
                (tmp / name).write_bytes(b"x")
            (tmp / "latest-linux-x86_64.json").write_bytes(b"{}")
            with self.assertRaises(release.ReleaseError):
                release.check_exact_asset_set(tmp, "1.2.3")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


class CommandLine(TempDirCase):
    def run_main(self, *argv: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = release.main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def test_the_new_subcommands_have_help(self) -> None:
        for command in ("updater-manifests", "check-updater-manifests", "fetch-previous-manifests", "platform-notes", "annotate-draft"):
            with self.subTest(command=command), contextlib.redirect_stdout(io.StringIO()), self.assertRaises(SystemExit) as raised:
                release.main([command, "--help"])
            self.assertEqual(raised.exception.code, 0)

    def test_updater_manifests_and_its_check_run_end_to_end(self) -> None:
        assets = self.make_assets((WINDOWS, LINUX))
        previous = self.make_previous(per_platform=(MACOS,))
        out = self.tmp / "manifests"
        common = ["--version", self.VERSION, "--dir", str(assets), "--platforms", "windows-x64,linux-x64", "--previous", str(previous)]
        code, stdout, stderr = self.run_main("updater-manifests", *common, "--notes-file", str(self.notes_file()),
                                              "--out", str(out), "--pub-date", "2026-02-02T00:00:00Z")
        self.assertEqual(code, 0, stderr)
        self.assertIn("written", stdout)
        self.assertIn("WARNING", stderr)
        self.assertEqual(self.names(out), ["latest-darwin-aarch64.json", "latest-linux-x86_64.json", "latest-windows-x86_64.json"])
        code, stdout, stderr = self.run_main("check-updater-manifests", *common, "--manifests", str(out))
        self.assertEqual(code, 0, stderr)
        self.assertTrue(stdout.startswith("PASS"))

    def test_an_invalid_platform_list_fails_closed(self) -> None:
        code, _, stderr = self.run_main("exact-set", "--dir", str(self.tmp), "--version", "1.2.3", "--platforms", "windows-x64,windows-x64")
        self.assertEqual(code, 1)
        self.assertTrue(stderr.startswith("FAIL:"))

    def test_platform_notes_prints_nothing_for_a_full_set(self) -> None:
        code, stdout, _ = self.run_main("platform-notes", "--version", "1.2.3", "--repo", "example/app")
        self.assertEqual((code, stdout), (0, ""))

    def test_the_existing_updater_manifest_command_keeps_its_behaviour(self) -> None:
        assets = self.make_assets()
        out = self.tmp / "latest.json"
        code, _, stderr = self.run_main("updater-manifest", "--version", "1.2.3", "--dir", str(assets), "--out", str(out),
                                        "--notes-file", str(self.notes_file()), "--pub-date", "2026-02-02T00:00:00Z")
        self.assertEqual(code, 0, stderr)
        self.assertEqual(set(json.loads(out.read_text(encoding="utf-8"))["platforms"]), set(release.UPDATER_PLATFORMS))
        code, _, stderr = self.run_main("check-updater-manifest", "--version", "1.2.3", "--dir", str(assets), "--latest-json", str(out))
        self.assertEqual(code, 0, stderr)


if __name__ == "__main__":
    unittest.main()
