# Release Process

For maintainers. A release starts when the promotion pull request from `development` into `main` is merged. Nobody runs it by hand. The workflow is [`release.yml`](https://github.com/JonHHH09/JobniacFastFs/blob/main/.github/workflows/release.yml).

The release is draft-first and per platform. A draft is created and verified, and only the `publish` job tags the merge commit and publishes it. Each platform is a lane (its tests, its build and its smoke test), and a release carries every platform whose lane is green, as one version, one tag and one release page. A red shared check, a failed version computation or SBOM, or no green platform leaves the draft unpublished.

## Trigger

`release.yml` runs in three situations, decided by its `mode` job: a pull request into `main` from `development` in this repository that is open (a dry run: nothing is released), the same pull request closed as merged (a release), and a manual run on `main` (a re-release of a platform, below). Anything else skips every job. The branch flow is in [Contributing Workflow](Contributing-Workflow.md).

Merge the promotion pull request only when its checks are green, `E2E (...)` included. A red platform check takes that platform out of the release, and the run ends red.

## Jobs

| Job | What it does |
|---|---|
| `mode` | Decides the mode, the commit and the selected platforms. |
| `checks-shared`, `checks-linux`, `checks-macos`, `checks-windows` | Call `ci.yml` on the merge commit, one scope each. The shared scope always runs; a platform's runs when it is selected. |
| `e2e-linux`, `e2e-windows` | Call `e2e.yml` on the merge commit for a selected platform. |
| `prepare` | Read-only. Checks that HEAD is the merge commit, resolves the version and tag, and refuses the placeholder updater public key. |
| `sbom` | Writes the software bill of materials once, recomputed and compared byte for byte. |
| `draft` | The only job that creates the draft. It checks the tag, prunes stale Windows installers and creates or reuses the single draft. |
| `build-windows`, `build-linux`, `build-macos` | One lane build each (`release-build.yml`), see below. They only build and hold no GitHub token. |
| `clean-machine` | Windows smoke on a fresh runner: install, launch, exercise MCP, uninstall, check residue. |
| `clean-machine-macos` | macOS smoke: mount the DMG, launch, exercise MCP and the PDF worker. |
| `plan` | Lists the green platforms; fails when there are none. |
| `attach` | Assembles the green platforms' asset set, writes and verifies `SHA256SUMS`, attaches the missing assets, then writes and attaches the updater manifests. |
| `attest` | Build provenance attestation. It reads the repository visibility and attests only a public repository (it logs the skip for a private one); `publish` needs it to succeed. |
| `verify-draft` | Final API check of the draft: tag, flags, target commit, exact asset set and digests, and every updater manifest. |
| `publish` | Tags the merge commit and publishes the draft. |
| `platform-issues` | Opens or updates one `release-platform` issue per missing platform, and closes it when a platform ships. |

The checks skip their heavy jobs when the promotion pull request already passed the same tree. See [Testing and CI](Testing-and-CI.md).

### Re-release

A platform that was left out is released again by a manual run on `main`: `gh workflow run release.yml --ref main -f platforms=macos-arm64`. It builds `main`'s current commit as the next patch version for the selected platforms only, with a new draft, tag and release. A published release is never edited. The other platforms keep their previous updater manifests.

### Build legs

| Platform | Runner | Output |
|---|---|---|
| Windows x64 | `windows-latest` | NSIS installer |
| Linux x64 | `ubuntu-24.04` | AppImage and deb |
| macOS Apple Silicon | `macos-latest` | DMG and an app archive for the updater |

Linux builds on 24.04 on purpose. The build host sets the glibc floor of the AppImage and deb. Every leg fetches the pinned PDFium, builds the CLI sidecar, generates licence notices, audits the built binaries, and reconciles the payload with the notices. The NSIS installer, the AppImage and the macOS app archive get an updater signature.

## Versioning

The release version is computed, not typed into the workflow.

- By default it is the patch after the highest stable `v*` tag. If the latest tag is `v0.1.2`, the next release is `0.1.3`.
- To raise the version, run this on `development` and merge the result:

  ```text
  python -I scripts/release/release.py bump X.Y.Z
  ```

  `X.Y.Z` can be a new minor or major version, or `X.Y.Z-dev.N` for a prerelease. A bumped version that is higher than the tags and not yet tagged is used as is.
- A rerun of the workflow on the same merge commit reuses the tag already on that commit. A re-release takes the next patch even though the commit is tagged.
- Nothing is committed back to the repository. Each build leg stamps the version into its own checkout.
- A stable version (`vX.Y.Z`) is published as the latest release, which the in-app updater reads. A `-dev.N` version is published as a prerelease and is never the latest.
- Release runs are serialized in one concurrency group.

`release.py check-versions` runs in CI and checks that the versions in `Cargo.toml`, `src-tauri/tauri.conf.json` and `package.json` agree and that `CHANGELOG.md` has notes for them.

## Release notes

The notes are the section of `CHANGELOG.md` for the version. If there is none, the release uses GitHub's generated notes. Write the changelog section before you promote.

## Signing

- **Updater key.** The in-app updater only accepts updates signed with the project key. The public key is committed in `src-tauri/tauri.release.conf.json`. The private key and its password are the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Only the build step reads them. Keep an offline backup of the private key. If it is lost, installed copies can never update.
- **Windows.** The installer is unsigned until a signing route is chosen. The release owner checklist lists the options.
- **macOS.** With the Apple Developer ID secrets set, the macOS leg signs and notarizes. Without them it builds an ad-hoc signed app. Set the repository variable `MACOS_SIGNING_REQUIRED` to exactly `1` once the secrets exist, so a missing secret fails the release instead of shipping an ad-hoc build.

The full list of secrets, names and gates is in the [release owner checklist](https://github.com/JonHHH09/JobniacFastFs/blob/main/docs/ops/release-owner-checklist.md). The scripted smokes are described in the [clean-machine smoke checklist](https://github.com/JonHHH09/JobniacFastFs/blob/main/docs/ops/clean-machine-smoke-checklist.md).

## If a job fails

- Read the failed step's log before you rerun.
- The draft stays unpublished. A rerun reuses the draft and the tag on the merge commit. The next promotion prunes stale Windows assets and rebuilds.
- Branch and merge settings are recorded in [branch-protection.md](https://github.com/JonHHH09/JobniacFastFs/blob/main/docs/ops/branch-protection.md).

## See also

- [Contributing Workflow](Contributing-Workflow.md)
- [Testing and CI](Testing-and-CI.md)
