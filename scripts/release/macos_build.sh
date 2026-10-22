#!/usr/bin/env bash
# Builds the macOS bundles (.app, .dmg and the updater archive) of the release leg, signed and notarized
# when the Developer ID secrets exist and ad-hoc signed otherwise.
#
# Run by the `build` job of release.yml on the macOS leg, instead of tauri-action: this step never sees the
# GitHub token, and the Apple secrets reach this step only. Inputs (environment):
#   BUNDLES                      the Tauri bundle list, for example `app,dmg`
#   MACOS_SIGNING_REQUIRED       exactly `1`: an ad-hoc build (no Developer ID secrets) fails instead of
#                                warning (the repository variable of the same name)
#   TAURI_SIGNING_PRIVATE_KEY    the updater signing key (and its _PASSWORD), read by `tauri build`
#   APPLE_CERTIFICATE            the base64 .p12 of the Developer ID Application certificate
#   APPLE_CERTIFICATE_PASSWORD   its password
#   APPLE_SIGNING_IDENTITY       for example `Developer ID Application: Name (TEAMID)`
#   Notarization, either the App Store Connect API key:
#     APPLE_API_ISSUER, APPLE_API_KEY (the key id), APPLE_API_PRIVATE_KEY (the contents of the .p8 file)
#   or an Apple ID:
#     APPLE_ID, APPLE_PASSWORD (an app-specific password), APPLE_TEAM_ID
#
# A secret that is not defined reaches the step as an empty string, which `tauri build` would read as
# "set", so every empty variable is unset first. Mode:
#   - no certificate and no notarization credential: ad-hoc build (src-tauri/tauri.macos.conf.json);
#   - certificate, password, identity and one complete notarization set: Developer ID build, notarized
#     and stapled by `tauri build`, without the ad-hoc-only entitlement;
#   - anything in between fails closed (a Developer ID app that is not notarized is refused by Gatekeeper).
# No secret is printed, and `set -x` is never on.
set -euo pipefail
set +x

: "${BUNDLES:?BUNDLES is required}"

apple_variables=(
  APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY
  APPLE_API_ISSUER APPLE_API_KEY APPLE_API_PRIVATE_KEY APPLE_API_KEY_PATH
  APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
)
for name in "${apple_variables[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    unset "$name"
  fi
done

defined() { [[ -n "${!1:-}" ]]; }

api_key_set=false
apple_id_set=false
if defined APPLE_API_ISSUER && defined APPLE_API_KEY && defined APPLE_API_PRIVATE_KEY; then api_key_set=true; fi
if defined APPLE_ID && defined APPLE_PASSWORD && defined APPLE_TEAM_ID; then apple_id_set=true; fi

signing=false
if defined APPLE_CERTIFICATE || defined APPLE_CERTIFICATE_PASSWORD || defined APPLE_SIGNING_IDENTITY \
  || defined APPLE_API_ISSUER || defined APPLE_API_KEY || defined APPLE_API_PRIVATE_KEY \
  || defined APPLE_ID || defined APPLE_PASSWORD || defined APPLE_TEAM_ID; then
  signing=true
fi

args=(--bundles "$BUNDLES" --config src-tauri/tauri.release.conf.json)
cleanup() {
  if [[ -n "${key_file:-}" ]]; then rm -f "$key_file"; fi
}
trap cleanup EXIT

if [[ "$signing" == true ]]; then
  if ! defined APPLE_CERTIFICATE || ! defined APPLE_CERTIFICATE_PASSWORD || ! defined APPLE_SIGNING_IDENTITY; then
    echo "::error::the Developer ID signing secrets are incomplete (need APPLE_CERTIFICATE, APPLE_CERTIFICATE_PASSWORD and APPLE_SIGNING_IDENTITY)" >&2
    exit 1
  fi
  if [[ "$api_key_set" != true && "$apple_id_set" != true ]]; then
    echo "::error::the notarization secrets are incomplete (need APPLE_API_ISSUER, APPLE_API_KEY and APPLE_API_PRIVATE_KEY, or APPLE_ID, APPLE_PASSWORD and APPLE_TEAM_ID)" >&2
    exit 1
  fi
  if [[ "$api_key_set" == true ]]; then
    # `tauri build` reads the key from a file. It lives in a private temporary file for this step only.
    umask 077
    key_file="$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/AuthKey.XXXXXX")"
    printf '%s\n' "$APPLE_API_PRIVATE_KEY" > "$key_file"
    export APPLE_API_KEY_PATH="$key_file"
    # Notarize with the API key only, even when an Apple ID is also defined.
    unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
  fi
  unset APPLE_API_PRIVATE_KEY
  # The ad-hoc entitlement (library validation off) is not needed: the bundler signs the PDFium library
  # with the same Developer ID identity as the app, so library validation passes.
  args+=(--config '{"bundle":{"macOS":{"entitlements":null}}}')
  echo "macOS build: Developer ID signing and notarization"
else
  # An ad-hoc app is only openable after the user allows it by hand.
  echo "::warning::the macOS app is built ad-hoc signed: no Developer ID secrets are defined, so users must allow it in Privacy & Security"
  if [[ "${MACOS_SIGNING_REQUIRED:-}" == "1" ]]; then
    echo "::error::MACOS_SIGNING_REQUIRED is 1 but the Developer ID secrets are missing (check their names): refusing an ad-hoc release build" >&2
    exit 1
  fi
  echo "macOS build: ad-hoc signature (no Developer ID secrets defined)"
fi

# Two steps, so that compilation (cargo build scripts, proc-macros, beforeBuildCommand) never sees a
# secret: `tauri build --no-bundle` compiles with none of the Apple or updater secrets in its
# environment, and `tauri bundle` (which compiles nothing) gets them only for its own process.
secret_names=("${apple_variables[@]}" TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD)
for name in "${secret_names[@]}"; do
  if [[ -n "${!name+x}" ]]; then
    printf -v "STASHED_$name" '%s' "${!name}"
    unset "$name"
  fi
done

pnpm tauri build --no-bundle --config src-tauri/tauri.release.conf.json

(
  for name in "${secret_names[@]}"; do
    stashed="STASHED_$name"
    if [[ -n "${!stashed+x}" ]]; then
      export "$name=${!stashed}"
    fi
  done
  exec pnpm tauri bundle "${args[@]}"
)
