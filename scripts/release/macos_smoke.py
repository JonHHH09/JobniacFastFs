"""Release smoke for the macOS (Apple Silicon) DMG and app archive.

Stdlib only (it imports the helpers of this folder). Run with
`python -I scripts/release/macos_smoke.py ...` on a fresh macOS runner. Each step prints PASS or
FAIL; the first failure stops the run and exits 1.

Steps: DMG and app-archive digests against the build leg's digests, mount the DMG and copy the app
out (the "installation"), bundle layout (PDFium in Contents/Frameworks, not in Resources),
`codesign --verify --deep --strict` of the copied app and of the unpacked updater archive (a
Developer ID build also `spctl --assess` and `stapler validate`, and one Team ID for the app and the
library), GUI launch with the smoke quit hook, CLI --version and --help, MCP over STDIO (initialize
and tools/list with 34 tools), and a PDF text extraction through the real worker path: the
bundled `jobniac-cli` re-executes itself as the sandboxed PDF worker, which loads the bundled
`libpdfium.dylib` from inside the app with no `JOBNIAC_PDFIUM_DIR` set.

The payload inventory is not compared with the app: signing rewrites the Mach-O files, so their digests
differ from the unsigned build output that the inventory describes.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path
from typing import Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))

import clean_machine_smoke as common  # noqa: E402
import release  # noqa: E402

APP_NAME = f"{release.PRODUCT}.app"
FIXTURE_NAME = "resume-simple.pdf"
FIXTURE_TEXT = "Jane Doe"
LIBRARY = "libpdfium.dylib"


class SmokeFailure(common.SmokeFailure):
    """A step failed; the message is printed without any path or response text."""


def command(arguments: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
    return common.run(arguments, **kwargs)  # type: ignore[arg-type]


def signing_details(path: Path) -> str:
    """The `codesign -dvv` report (it is written to stderr)."""
    completed = command(["/usr/bin/codesign", "-dvv", str(path)])
    return completed.stdout + completed.stderr


def team_identifier(path: Path) -> str:
    match = re.search(r"^TeamIdentifier=(.+)$", signing_details(path), re.MULTILINE)
    return match.group(1).strip() if match else ""


def is_developer_id(app: Path) -> bool:
    return "Authority=Developer ID Application" in signing_details(app)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--dmg", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True, help="the updater app archive of the leg")
    parser.add_argument("--leg-digests", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--require-developer-id", action="store_true", help="fail unless the app has a Developer ID signature")
    parser.add_argument("--pdf-fixture", type=Path, required=True, help="a synthetic PDF with the text 'Jane Doe'")
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args(argv)

    mount = args.work / "mount"
    install_dir = args.work / "install"
    unpacked = args.work / "archive"
    import_root = args.work / "import"
    data_dir = args.work / "data"
    app = install_dir / APP_NAME
    macos_dir = app / "Contents" / "MacOS"
    gui = macos_dir / "jobniac"
    cli = macos_dir / "jobniac-cli"
    library = app / "Contents" / "Frameworks" / LIBRARY
    state = {"mounted": False}

    def smoke_env(**extra: str) -> dict[str, str]:
        return common.smoke_env(args.work, **extra)

    def check_digests() -> None:
        recorded = {}
        for line in args.leg_digests.read_text(encoding="utf-8").splitlines():
            digest, _, name = line.partition("  ")
            recorded[name] = digest
        for artifact in (args.dmg, args.archive):
            if recorded.get(artifact.name) != release.sha256_file(artifact):
                raise SmokeFailure("an artifact digest differs from the build leg")

    def install() -> None:
        data_dir.mkdir(parents=True, exist_ok=True)
        mount.mkdir(parents=True, exist_ok=True)
        install_dir.mkdir(parents=True, exist_ok=True)
        # The DMG shows the licence agreement (bundle.licenseFile); the attach reads "Y" to accept it.
        attach = command(
            ["/usr/bin/hdiutil", "attach", "-nobrowse", "-readonly", "-noverify", "-mountpoint", str(mount), str(args.dmg)],
            stdin="Y\n",
        )
        if attach.returncode != 0:
            raise SmokeFailure("the DMG did not mount")
        state["mounted"] = True
        if not (mount / APP_NAME).is_dir():
            raise SmokeFailure("the DMG holds no app")
        if command(["/usr/bin/ditto", str(mount / APP_NAME), str(app)]).returncode != 0:
            raise SmokeFailure("the app could not be copied out of the DMG")
        detach()

    def detach() -> None:
        if state["mounted"]:
            state["mounted"] = False
            command(["/usr/bin/hdiutil", "detach", "-force", str(mount)])

    def layout() -> None:
        for path in (gui, cli, library):
            if not path.is_file():
                raise SmokeFailure("a bundle file is missing")
        if (app / "Contents" / "Resources" / "pdfium").exists():
            raise SmokeFailure("PDFium sits in Resources, where it is not signed as code")
        minimum = command(["/usr/bin/defaults", "read", str(app / "Contents" / "Info"), "LSMinimumSystemVersion"])
        if minimum.returncode != 0 or not minimum.stdout.strip():
            raise SmokeFailure("the bundle declares no minimum system version")

    def signature() -> None:
        if command(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)]).returncode != 0:
            raise SmokeFailure("codesign verification failed")
        if is_developer_id(app):
            sys.stdout.write("INFO: Developer ID signature\n")
            if team_identifier(app) in ("", "not set") or team_identifier(app) != team_identifier(library):
                raise SmokeFailure("the app and the library have no common Team ID")
            if command(["/usr/sbin/spctl", "--assess", "--type", "execute", str(app)]).returncode != 0:
                raise SmokeFailure("Gatekeeper refuses the app")
            if command(["/usr/bin/xcrun", "stapler", "validate", str(app)]).returncode != 0:
                raise SmokeFailure("the notarization ticket is not stapled")
        elif args.require_developer_id:
            raise SmokeFailure("a Developer ID signature is required and the app is ad-hoc signed")
        else:
            sys.stdout.write("INFO: ad-hoc signature (no Developer ID secrets were configured)\n")

    def archive() -> None:
        unpacked.mkdir(parents=True, exist_ok=True)
        with tarfile.open(args.archive) as bundle:
            members = bundle.getnames()
            if not all(name == APP_NAME or name.startswith(f"{APP_NAME}/") for name in members):
                raise SmokeFailure("the archive holds more than the app")
            bundle.extractall(unpacked, filter="tar")
        extracted = unpacked / APP_NAME
        if not (extracted / "Contents" / "Frameworks" / LIBRARY).is_file():
            raise SmokeFailure("the archive has no PDFium library")
        if command(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(extracted)]).returncode != 0:
            raise SmokeFailure("codesign verification of the archive failed")

    def pubkey() -> None:
        release.check_binary_pubkey(gui)

    def launch() -> None:
        env = smoke_env(JOBNIAC_SMOKE_QUIT_AFTER_LOAD="1")
        if command([str(gui)], env=env).returncode != 0:
            raise SmokeFailure("the application did not load and exit 0")

    def cli_checks() -> None:
        version = command([str(cli), "--version"], env=smoke_env())
        if version.returncode != 0 or args.version not in version.stdout:
            raise SmokeFailure("--version does not print the release version")
        if command([str(cli), "--help"], env=smoke_env()).returncode != 0:
            raise SmokeFailure("--help failed")

    def mcp() -> None:
        completed = command([str(cli), "mcp"], env=smoke_env(), stdin=common.mcp_requests())
        if completed.returncode != 0:
            raise SmokeFailure("mcp exited with an error")
        messages = [json.loads(line) for line in completed.stdout.splitlines() if line.strip()]
        by_id = {message["id"]: message for message in messages if "id" in message}
        if by_id.get(1, {}).get("result", {}).get("protocolVersion") != common.PROTOCOL_VERSION:
            raise SmokeFailure("protocol version was not negotiated")
        tools = by_id.get(2, {}).get("result", {}).get("tools")
        if not isinstance(tools, list) or len(tools) != common.EXPECTED_TOOL_COUNT:
            raise SmokeFailure("tool count differs from the default catalog")

    def pdf_worker() -> None:
        import_root.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(args.pdf_fixture, import_root / FIXTURE_NAME)
        requests = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": common.PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "macos-smoke", "version": "1"},
            }},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
                "name": "extract_pdf_text",
                "arguments": {"request": {"path": str(import_root / FIXTURE_NAME), "maxCharacters": 500, "includePages": False}},
            }},
        ]
        # No JOBNIAC_PDFIUM_DIR (clean_env removes every JOBNIAC_* variable): the library must be found
        # inside the bundle, and the worker child must load it under the hardened runtime.
        env = smoke_env(JOBNIAC_DOCUMENT_IMPORT_ROOT=str(import_root))
        completed = command([str(cli), "mcp"], env=env, stdin="".join(json.dumps(request) + "\n" for request in requests))
        if completed.returncode != 0:
            raise SmokeFailure("mcp exited with an error")
        by_id = {}
        for line in completed.stdout.splitlines():
            if line.strip():
                message = json.loads(line)
                if "id" in message:
                    by_id[message["id"]] = message
        result = by_id.get(2, {}).get("result", {})
        content = result.get("structuredContent") or {}
        if result.get("isError") or FIXTURE_TEXT not in str(content.get("text", "")):
            raise SmokeFailure("the bundled PDF worker did not extract the fixture text")

    steps: list[tuple[str, Callable[[], None]]] = [
        ("DMG and app archive digests match the build leg", check_digests),
        ("mount the DMG and copy the app out", install),
        ("bundle layout: PDFium in Contents/Frameworks", layout),
        ("code signature of the app", signature),
        ("updater archive unpacks to a verified app", archive),
        ("app embeds the updater public key once", pubkey),
        ("GUI launches and exits 0 with the smoke quit hook", launch),
        ("CLI --version and --help", cli_checks),
        ("MCP over STDIO: initialize and tools/list (34 tools)", mcp),
        ("PDF text through the bundled worker and the bundled PDFium", pdf_worker),
    ]
    try:
        for name, action in steps:
            try:
                action()
            except (SmokeFailure, release.ReleaseError, OSError, subprocess.SubprocessError, tarfile.TarError, ValueError, KeyError) as error:
                sys.stdout.write(f"FAIL: {name} ({common.describe(error)})\n")
                return 1
            sys.stdout.write(f"PASS: {name}\n")
    finally:
        detach()
    return 0


if __name__ == "__main__":
    sys.exit(main())
