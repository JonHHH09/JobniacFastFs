# Updates

## The in-app updater

JobniacFast checks GitHub for a newer signed release. It reads `latest.json` from the latest release, at `https://github.com/JonHHH09/JobniacFastFs/releases/latest/download/latest.json`.

In Settings, the Updates card has:

- **Check for updates**: asks GitHub when you choose it.
- **Check for updates after startup**: off by default. When on, the app asks GitHub once each time it starts. The choice is remembered across restarts.

When a newer version exists, an "Update available" banner shows what changed. Choose **Install and restart** and confirm. The update is downloaded, checked against the publisher's signature, installed, and the app restarts. The app refuses an update that is not signed with the project's key. Unsaved drafts are lost on restart, so save your work first.

Your data stays where it is. Updates do not touch the database file.

Limits:

- A copy installed from a `.deb` package cannot update itself. The app links to the releases page: download the new package and install it.
- A development build reports that updates are unavailable.

## Versions

- A stable release has a version `X.Y.Z` and the tag `vX.Y.Z`. It becomes the latest release, which is the one the updater reads.
- Each merge of `development` into `main` publishes the next patch version automatically, after the checks pass.
- Versions of the form `X.Y.Z-dev.N` are prereleases. They are never the latest release, so the updater never offers them. Install one by hand from the releases page if you want to try it.

The [CHANGELOG](https://github.com/JonHHH09/JobniacFastFs/blob/main/CHANGELOG.md) lists what changed in each version.

## Related

- [Installation](Installation.md)
- [Troubleshooting](Troubleshooting.md)
