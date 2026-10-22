#!/usr/bin/env bash
# Downloads the pinned PDFium binary into vendor/pdfium/<platform>/ (Linux, Git Bash or macOS).
#
# Source: bblanchon/pdfium-binaries, release tag chromium/7881 (PDFium 151.0.7881.0). Each archive is checked
# against its pinned SHA-256 before it is extracted, and the extracted library is checked against its pinned
# SHA-256 before it is placed. Any mismatch fails closed: nothing is written under vendor/.
#
# Only the library, the upstream LICENSE and the licenses/ notice directory are kept. Binaries are never
# committed (vendor/ is gitignored). The fetch is idempotent: a platform whose library already matches the
# pinned SHA-256 is skipped.
#
# The code and tests find the library through JOBNIAC_PDFIUM_DIR, which must point at vendor/pdfium/<platform>/.
# Change the tag and the digests together.
#
# Usage: scripts/fetch-pdfium.sh [windows-x64|linux-x64|mac-arm64|mac-x64|all]   (default: all, which is
# windows-x64, linux-x64 and mac-arm64, the release platforms; mac-x64 is fetched by name)
set -euo pipefail

readonly RELEASE_BASE_URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881"

# Sets ARCHIVE, ARCHIVE_SHA256, LIBRARY_PATH and LIBRARY_SHA256 for one platform.
pin_for() {
  case "$1" in
    windows-x64)
      ARCHIVE="pdfium-win-x64.tgz"
      ARCHIVE_SHA256="73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08"
      LIBRARY_PATH="bin/pdfium.dll"
      LIBRARY_SHA256="79d4676b656cfb1abcea88f9ade3b4b0826c5200382db5f4ec72a636c598c118"
      ;;
    linux-x64)
      ARCHIVE="pdfium-linux-x64.tgz"
      ARCHIVE_SHA256="1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d"
      LIBRARY_PATH="lib/libpdfium.so"
      LIBRARY_SHA256="f728930966f503652b92acc89b9374a2eeca00ce42e26dccd3e4b5c5161b2d64"
      ;;
    mac-arm64)
      ARCHIVE="pdfium-mac-arm64.tgz"
      ARCHIVE_SHA256="52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40"
      LIBRARY_PATH="lib/libpdfium.dylib"
      LIBRARY_SHA256="1bc45b15466b34cef96641ce25c77a876e70010c6b114f909dda2f5325fc5bd7"
      ;;
    mac-x64)
      ARCHIVE="pdfium-mac-x64.tgz"
      ARCHIVE_SHA256="6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b"
      LIBRARY_PATH="lib/libpdfium.dylib"
      LIBRARY_SHA256="4eaad6c3e8d786cf6f66a45d7d014edf5c65f372f98c3070e66595ebb50e43d9"
      ;;
    *)
      echo "unknown platform: $1 (expected windows-x64, linux-x64, mac-arm64, mac-x64 or all)" >&2
      exit 2
      ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  else
    shasum -a 256 "$1" | awk '{ print $1 }'
  fi
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
vendor_root="$repo_root/vendor/pdfium"
requested="${1:-all}"

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "tar is required" >&2; exit 1; }

if [[ "$requested" == "all" ]]; then
  # The release platforms only (scripts/release/release.py); Intel macOS is not one and is fetched by name.
  platforms=(windows-x64 linux-x64 mac-arm64)
else
  platforms=("$requested")
fi

for platform in "${platforms[@]}"; do
  pin_for "$platform"
  dest="$vendor_root/$platform"
  library_leaf="$(basename "$LIBRARY_PATH")"
  library_in_dest="$dest/$library_leaf"

  if [[ -f "$library_in_dest" ]] && [[ "$(sha256_of "$library_in_dest")" == "$LIBRARY_SHA256" ]]; then
    echo "[$platform] present and verified: $library_in_dest"
    continue
  fi

  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  archive_path="$work/$ARCHIVE"
  extract="$work/extract"
  stage="$work/stage"
  mkdir -p "$extract" "$stage"

  echo "[$platform] downloading $ARCHIVE"
  curl --fail --silent --show-error --location -o "$archive_path" "$RELEASE_BASE_URL/$ARCHIVE"

  archive_digest="$(sha256_of "$archive_path")"
  if [[ "$archive_digest" != "$ARCHIVE_SHA256" ]]; then
    echo "SHA-256 mismatch for $ARCHIVE: expected $ARCHIVE_SHA256, got $archive_digest. Nothing was installed." >&2
    exit 1
  fi

  tar -xzf "$archive_path" -C "$extract"

  library_in_archive="$extract/$LIBRARY_PATH"
  for required in "$library_in_archive" "$extract/LICENSE" "$extract/licenses"; do
    if [[ ! -e "$required" ]]; then
      echo "$ARCHIVE does not contain the expected member: $required" >&2
      exit 1
    fi
  done

  library_digest="$(sha256_of "$library_in_archive")"
  if [[ "$library_digest" != "$LIBRARY_SHA256" ]]; then
    echo "SHA-256 mismatch for $LIBRARY_PATH in $ARCHIVE: expected $LIBRARY_SHA256, got $library_digest. Nothing was installed." >&2
    exit 1
  fi

  # Stage the kept members first, then replace the platform directory in one step.
  cp "$library_in_archive" "$stage/$library_leaf"
  cp "$extract/LICENSE" "$stage/LICENSE"
  cp -R "$extract/licenses" "$stage/licenses"

  rm -rf "$dest"
  mkdir -p "$vendor_root"
  mv "$stage" "$dest"
  rm -rf "$work"
  trap - EXIT
  echo "[$platform] installed $library_in_dest (sha256 $library_digest)"
done
