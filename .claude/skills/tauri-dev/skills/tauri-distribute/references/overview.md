# Distribute

> Source: https://v2.tauri.app/distribute/ (tauri-docs `4b74da6`, 2026-10-13). Local copy; site components flattened, links point to the live site.
>
> Information on the tooling you need to distribute your application either to the platform app stores or as platform-specific installers

Tauri provides the tooling you need to distribute your application either to the platform app stores or as platform-specific installers.

## Building

Tauri builds your application directly from its CLI via the `build`, `android build` and `ios build` commands.

```sh
# npm
npm run tauri build
# yarn
yarn tauri build
# pnpm
pnpm tauri build
# deno
deno task tauri build
# bun
bun tauri build
# cargo
cargo tauri build
```

See the [distributing](#distributing) section to learn more about the configuration options available for each bundle
and how to distribute them to your users.

:::note
Most platforms requires code signing. See the [signing](#signing) section for more information.
:::

### Bundling

By default the `build` command automatically bundles your application for the configured formats.

If you need further customization on how the platform bundles are generated, you can split the build and bundle steps:

```sh
# npm
npm run tauri build -- --no-bundle
# bundle for distribution outside the macOS App Store
npm run tauri bundle -- --bundles app,dmg
# bundle for App Store distribution
npm run tauri bundle -- --bundles app --config src-tauri/tauri.appstore.conf.json
# yarn
yarn tauri build --no-bundle
# bundle for distribution outside the macOS App Store
yarn tauri bundle --bundles app,dmg
# bundle for App Store distribution
yarn tauri bundle --bundles app --config src-tauri/tauri.appstore.conf.json
# pnpm
pnpm tauri build --no-bundle
# bundle for distribution outside the macOS App Store
pnpm tauri bundle --bundles app,dmg
# bundle for App Store distribution
pnpm tauri bundle --bundles app --config src-tauri/tauri.appstore.conf.json
# deno
deno task tauri build --no-bundle
# bundle for distribution outside the macOS App Store
deno task tauri bundle --bundles app,dmg
# bundle for App Store distribution
deno task tauri bundle --bundles app --config src-tauri/tauri.appstore.conf.json
# bun
bun tauri build --no-bundle
# bundle for distribution outside the macOS App Store
bun tauri bundle --bundles app,dmg
# bundle for App Store distribution
bun tauri bundle --bundles app --config src-tauri/tauri.appstore.conf.json
# cargo
cargo tauri build --no-bundle
# bundle for distribution outside the macOS App Store
cargo tauri bundle --bundles app,dmg
# bundle for App Store distribution
cargo tauri bundle --bundles app --config src-tauri/tauri.appstore.conf.json
```

## Versioning

Your application version can be defined in the [`tauri.conf.json > version`] configuration option,
which is the recommended way for managing the app version. If that config value is not set,
Tauri uses the `package > version` value from your `src-tauri/Cargo.toml` file instead.

:::note
Some platforms have some limitations and special cases for the version string.
See the individual distribution documentation pages for more information.
:::

## Signing

Code signing enhances the security of your application by applying a digital signature to your
application's executables and bundles, validating your identity of the provider of your application.

Signing is required on most platforms. See the documentation for each platform for more information.

  - [macOS](https://v2.tauri.app/distribute/sign/macos/): Code signing and notarization for macOS apps
  - [Windows](https://v2.tauri.app/distribute/sign/windows/): Code signing Windows installers
  - [Linux](https://v2.tauri.app/distribute/sign/linux/): Code signing Linux packages
  - [Android](https://v2.tauri.app/distribute/sign/android/): Code signing for Android
  - [iOS](https://v2.tauri.app/distribute/sign/ios/): Code signing for iOS

## Distributing

Learn how to distribute your application for each platform.

### Linux

For Linux you can distribute your app using the Debian package, Snap, AppImage, Flatpak, RPM or Arch User Repository (AUR) formats.

  - [AppImage](https://v2.tauri.app/distribute/appimage/): Distribute as an AppImage
  - [AUR](https://v2.tauri.app/distribute/aur/): Publishing To The Arch User Repository
  - [Debian](https://v2.tauri.app/distribute/debian/): Distribute as a Debian package
  {/*  - [Flathub](https://v2.tauri.app/distribute/flatpak/): Distribute as a Flatpak */}
  - [RPM](https://v2.tauri.app/distribute/rpm/): Distribute as an RPM package
  - [Snapcraft](https://v2.tauri.app/distribute/snapcraft/): Distribute on Snapcraft.io

[Code signing](https://v2.tauri.app/distribute/sign/linux/)

### macOS

For macOS you can either distribute your application directly to the App Store or ship a DMG installer as direct download.
Both methods requires code signing, and distributing outside the App Store also requires notarization.

  - [App Bundle](https://v2.tauri.app/distribute/macos-application-bundle/): Distribute macOS apps as an App Bundle
  - [App Store](https://v2.tauri.app/distribute/app-store/): Distribute iOS and macOS apps to the App Store
  - [DMG](https://v2.tauri.app/distribute/dmg/): Distribute macOS apps as Apple Disk Images

[Code signing and notarization](https://v2.tauri.app/distribute/sign/macos/)

### Windows

Learn how to distribute to the Microsoft Store or configure a Windows installer.

  - [Microsoft Store](https://v2.tauri.app/distribute/microsoft-store/): Distribute Windows apps to the Microsoft Store
  - [Windows Installer](https://v2.tauri.app/distribute/windows-installer/): Distribute installers for Windows

[Code signing](https://v2.tauri.app/distribute/sign/windows/)

### Android

Distribute your Android application to Google Play.

  - [Google Play](https://v2.tauri.app/distribute/google-play/): Distribute Android apps to Google Play

[Code signing](https://v2.tauri.app/distribute/sign/android/)

### iOS

Learn how to upload your application to the App Store.

  - [App Store](https://v2.tauri.app/distribute/app-store/): Distribute iOS and macOS apps to the App Store

[Code signing](https://v2.tauri.app/distribute/sign/ios/)

### Cloud Services

Distribute your application to Cloud services that globally distribute your application and support auto updates out of the box.

  - [CrabNebula Cloud](https://v2.tauri.app/distribute/crabnebula-cloud/): Distribute your app using CrabNebula

[`tauri.conf.json > version`]: /reference/config/#version
