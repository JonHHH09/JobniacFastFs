#!/usr/bin/env bash
# Install the PostgreSQL 18 server binaries on an Ubuntu CI runner from the PostgreSQL apt
# repository. The repository signing key is accepted only if its fingerprint matches the
# published PostgreSQL Global Development Group key. No service is started; the test harness
# (jobniac-testkit pg-test) owns a disposable cluster.
set -euo pipefail

readonly EXPECTED_FINGERPRINT="B97B0AFCAA1A47F044F244A07FCC7D46ACCC4CF8"
readonly KEYRING="/usr/share/keyrings/postgresql-archive-keyring.gpg"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

curl --fail --silent --show-error --location \
  https://www.postgresql.org/media/keys/ACCC4CF8.asc -o "$workdir/key.asc"
gpg --batch --dearmor -o "$workdir/key.gpg" "$workdir/key.asc"

actual="$(gpg --batch --show-keys --with-colons "$workdir/key.gpg" | awk -F: '$1 == "fpr" { print $10; exit }')"
if [[ "$actual" != "$EXPECTED_FINGERPRINT" ]]; then
  echo "PostgreSQL signing key fingerprint mismatch" >&2
  exit 1
fi

sudo install -m 0644 "$workdir/key.gpg" "$KEYRING"
codename="$(. /etc/os-release && echo "$VERSION_CODENAME")"
echo "deb [signed-by=$KEYRING] https://apt.postgresql.org/pub/repos/apt ${codename}-pgdg main" \
  | sudo tee /etc/apt/sources.list.d/pgdg.list > /dev/null
sudo apt-get update
sudo apt-get install -y --no-install-recommends postgresql-18
# The package starts a default cluster; the tests never use it.
sudo systemctl stop postgresql || true
/usr/lib/postgresql/18/bin/postgres --version
