# Installation

JobniacFast is installed from a release on the [GitHub Releases page](https://github.com/JonHHH09/JobniacFastFs/releases). Download the file for your system, check it against `SHA256SUMS`, then install it.

## Requirements

- Windows 11 (x64), Linux (x86-64) or macOS 14 or later on Apple Silicon. Intel Macs are not supported.
- Linux packages are built on Ubuntu 24.04, so they need Ubuntu 24.04 or a newer distribution with an equal or newer glibc.
- Windows needs WebView2. Windows 11 includes it, and the installer installs it if it is missing.

## Files in a release

| System | File |
|---|---|
| Windows | `JobniacFast_<version>_x64-setup.exe` (NSIS installer) |
| Linux | `JobniacFast_<version>_amd64.AppImage` and `JobniacFast_<version>_amd64.deb` |
| macOS (Apple Silicon) | `JobniacFast_<version>_aarch64.dmg` |
| All | `SHA256SUMS`, a software bill of materials and third-party notices |

Download `SHA256SUMS` next to the installer.

## Check the download

Compare the SHA-256 hash of the file with the line for it in `SHA256SUMS`. If the two differ, do not run the file.

Windows (PowerShell):

```powershell
Get-FileHash -Algorithm SHA256 .\JobniacFast_<version>_x64-setup.exe
Select-String -Path .\SHA256SUMS -Pattern 'x64-setup'
```

Linux:

```text
sha256sum -c SHA256SUMS --ignore-missing
```

macOS:

```text
shasum -a 256 -c SHA256SUMS --ignore-missing
```

### Build provenance

When the repository is public, each release carries a build provenance attestation. You can check that a file was built by the project's release workflow with the GitHub CLI:

```text
gh attestation verify <file> --repo JonHHH09/JobniacFastFs
```

## Windows

1. Run the installer. It installs for your user only and does not ask for administrator rights.
2. The app is installed to `%LOCALAPPDATA%\JobniacFast`.

The Windows installer is not code-signed. Windows shows "Windows protected your PC" (SmartScreen). After you have checked the hash, click **More info**, then **Run anyway**. See [Troubleshooting](Troubleshooting.md).

## Linux

- AppImage: make the file executable (`chmod +x`) and run it.
- `.deb`: install it with your package manager, for example `sudo apt install ./JobniacFast_<version>_amd64.deb`.

A copy installed from the `.deb` cannot update itself. Download the new package from the releases page instead (see [Updates](Updates.md)).

## macOS (Apple Silicon)

1. Open the DMG and drag JobniacFast to Applications.
2. Open it from Applications.

A release signed with a Developer ID certificate and notarized by Apple opens normally. Until that is set up, the app is only ad-hoc signed, and macOS says it cannot verify the app. After you have checked the hash, open System Settings, Privacy & Security, and choose **Open Anyway** for JobniacFast. You can also run:

```text
xattr -dr com.apple.quarantine /Applications/JobniacFast.app
```

The release notes say which kind of signature a release has.

## Uninstall

- Windows: Settings, Apps, JobniacFast, Uninstall, or run `uninstall.exe` in the install folder.
- macOS: drag JobniacFast from Applications to the Bin.
- Linux: delete the AppImage, or remove the `.deb` with your package manager.

Uninstalling does not delete your data. It is stored outside the install folder; see [Data and Privacy](Data-and-Privacy.md) for the location and how to back it up or remove it.

## Next

[Getting Started](Getting-Started.md)
