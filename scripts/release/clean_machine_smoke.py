"""Clean-machine smoke for the Windows NSIS installer.

Stdlib only (it imports the release helpers of this folder). Run with
`python -I scripts/release/clean_machine_smoke.py ...` on a fresh runner. Each step prints PASS or
FAIL; the first failure stops the run and exits 1.

Steps: installer digest against the build leg's digest, silent per-user install, staged payload
against the installed tree, the updater public key in the installed app, GUI launch with the smoke
quit hook, CLI --version and --help, MCP over STDIO (initialize and tools/list with 34 tools; stdout
carries only JSON-RPC), silent uninstall, removal check and the per-user residue check.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable

sys.path.insert(0, str(Path(__file__).resolve().parent))

import release  # noqa: E402

# The protocol version of the MCP catalog's initialize result (crates/mcp/src/catalog/initialize.json).
PROTOCOL_VERSION = "2025-11-25"
EXPECTED_TOOL_COUNT = 34
TIMEOUT_SECONDS = 120
WEBVIEW_DATA_FOLDER = "EBWebView"


class SmokeFailure(Exception):
    """A step failed; the message is printed without any path or response text."""


def describe(error: BaseException) -> str:
    """The error class, with the message when it carries no path or response text.

    SmokeFailure and ReleaseError messages are fixed texts; an OSError gives its strerror only (the
    filename is left out).
    """
    if isinstance(error, (SmokeFailure, release.ReleaseError)):
        return f"{error.__class__.__name__}: {error}"
    if isinstance(error, OSError) and error.strerror:
        return f"{error.__class__.__name__}: {error.strerror}"
    return error.__class__.__name__


def per_user_roots() -> list[Path]:
    return [Path(os.environ[name]) for name in ("LOCALAPPDATA", "APPDATA") if os.environ.get(name)]


def root_entries() -> set[str]:
    entries: set[str] = set()
    for root in per_user_roots():
        if root.is_dir():
            entries.update(f"{root}|{entry.name}" for entry in root.iterdir())
    return entries


def clean_env() -> dict[str, str]:
    """The environment of a child process with every JOBNIAC_* variable removed."""
    return {key: value for key, value in os.environ.items() if not key.upper().startswith("JOBNIAC_")}


def smoke_env(work: Path, **extra: str) -> dict[str, str]:
    """clean_env() plus a database under `work/data`, so a run never writes the per-user default.

    The database stays outside the install directory (the uninstall waits for that directory to be
    empty) and the path is absolute, as JOBNIAC_DB_PATH requires.
    """
    env = clean_env()
    env["JOBNIAC_DB_PATH"] = str(work.resolve() / "data" / "smoke.sqlite3")
    env.update(extra)
    return env


def run(command: list[str], env: dict[str, str] | None = None, stdin: str | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command, input=stdin, capture_output=True, encoding="utf-8", errors="replace", env=env,
        timeout=TIMEOUT_SECONDS, check=False,
    )


def wait_for(predicate: Callable[[], bool], message: str) -> None:
    deadline = time.monotonic() + TIMEOUT_SECONDS
    while not predicate():
        if time.monotonic() > deadline:
            raise SmokeFailure(message)
        time.sleep(1)


def mcp_requests() -> str:
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "clean-machine-smoke", "version": "1"},
        }},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
    ]
    return "".join(json.dumps(request) + "\n" for request in requests)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--installer", type=Path, required=True)
    parser.add_argument("--leg-digests", type=Path, required=True)
    parser.add_argument("--inventory", type=Path, required=True, help="the staged Windows payload inventory")
    parser.add_argument("--version", required=True)
    parser.add_argument("--identifier", help="bundle identifier (default: src-tauri/tauri.conf.json)")
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args(argv)
    if not args.identifier:
        config = json.loads((release.REPO_ROOT / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))
        args.identifier = str(config["identifier"])

    install_dir = args.work / "install"
    before = root_entries()
    install_dir_text = str(install_dir)
    cli = install_dir / "jobniac-cli.exe"
    gui = install_dir / "jobniac.exe"

    def check_digest() -> None:
        recorded = {}
        for line in args.leg_digests.read_text(encoding="utf-8").splitlines():
            digest, _, name = line.partition("  ")
            recorded[name] = digest
        if recorded.get(args.installer.name) != release.sha256_file(args.installer):
            raise SmokeFailure("installer digest differs from the build leg")

    def install() -> None:
        install_dir.mkdir(parents=True, exist_ok=True)
        (args.work / "data").mkdir(parents=True, exist_ok=True)
        completed = run([str(args.installer), "/S", f"/D={install_dir_text}"])
        if completed.returncode != 0:
            raise SmokeFailure("silent install failed")
        wait_for(gui.is_file, "installed executable is missing")

    def tree() -> None:
        release.compare_installed(args.inventory, install_dir, release.INSTALLED_ONLY_ALLOWLIST)

    def pubkey() -> None:
        release.check_binary_pubkey(gui)

    def launch() -> None:
        env = smoke_env(args.work, JOBNIAC_SMOKE_QUIT_AFTER_LOAD="1")
        if run([str(gui)], env=env).returncode != 0:
            raise SmokeFailure("the application did not load and exit 0")

    def cli_checks() -> None:
        version = run([str(cli), "--version"], env=smoke_env(args.work))
        if version.returncode != 0 or args.version not in version.stdout:
            raise SmokeFailure("--version does not print the release version")
        if run([str(cli), "--help"], env=smoke_env(args.work)).returncode != 0:
            raise SmokeFailure("--help failed")

    def mcp() -> None:
        completed = run([str(cli), "mcp"], env=smoke_env(args.work), stdin=mcp_requests())
        if completed.returncode != 0:
            raise SmokeFailure("mcp exited with an error")
        messages = [json.loads(line) for line in completed.stdout.splitlines() if line.strip()]
        by_id = {message["id"]: message for message in messages if "id" in message}
        if by_id.get(1, {}).get("result", {}).get("protocolVersion") != PROTOCOL_VERSION:
            raise SmokeFailure("protocol version was not negotiated")
        tools = by_id.get(2, {}).get("result", {}).get("tools")
        if not isinstance(tools, list) or len(tools) != EXPECTED_TOOL_COUNT:
            raise SmokeFailure("tool count differs from the default catalog")

    def uninstall() -> None:
        uninstaller = install_dir / "uninstall.exe"
        if not uninstaller.is_file():
            raise SmokeFailure("uninstaller is missing")
        run([str(uninstaller), "/S"])
        wait_for(lambda: not install_dir.exists() or not any(install_dir.iterdir()),
                 "uninstall did not remove the installed files")

    def residue() -> None:
        if install_dir.exists():
            raise SmokeFailure("the installed tree remains after uninstall")
        for item in sorted(root_entries() - before):
            root, _, name = item.partition("|")
            lowered = name.lower()
            if lowered == args.identifier.lower():
                # The WebView2 user-data folder is the one entry that may remain.
                if (Path(root) / name / WEBVIEW_DATA_FOLDER).is_dir():
                    continue
                raise SmokeFailure("an identifier folder remains without the WebView2 data folder")
            if "jobniac" in lowered:
                raise SmokeFailure("a Jobniac entry remains in a per-user root")

    steps: list[tuple[str, Callable[[], None]]] = [
        ("installer digest matches the build leg", check_digest),
        ("silent per-user install", install),
        ("installed tree matches the staged payload", tree),
        ("installed app embeds the updater public key once", pubkey),
        ("GUI launches and exits 0 with the smoke quit hook", launch),
        ("CLI --version and --help", cli_checks),
        ("MCP over STDIO: initialize and tools/list (34 tools)", mcp),
        ("silent uninstall", uninstall),
        ("per-user residue check", residue),
    ]
    for name, action in steps:
        try:
            action()
        except (SmokeFailure, release.ReleaseError, OSError, subprocess.SubprocessError, ValueError, KeyError) as error:
            sys.stdout.write(f"FAIL: {name} ({describe(error)})\n")
            return 1
        sys.stdout.write(f"PASS: {name}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
