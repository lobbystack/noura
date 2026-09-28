# How do I publish a Noura release?

Push a version tag, and the release workflow builds signed installers for macOS, Windows, and Linux. Installed copies of Noura then find the new version and update themselves. This guide covers the one-time setup and each release.

## Set up signing once

The updater only installs bundles signed with the Noura updater key. The public key lives in `src-tauri/tauri.conf.json`. Keep the private key outside the repository.

Add the private key to the repository secrets:

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/noura-updater.key
```

If you protected the key with a password, also set `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Back up the private key. If you lose it, installed apps can't accept future updates, and users must reinstall by hand.

To sign and notarize the macOS app with an Apple Developer ID, add these secrets: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`. Without them, the workflow signs the app ad hoc, and macOS asks each user to approve it in **System Settings** > **Privacy & Security** on first launch.

## Publish a version

1. Set the same version in `src-tauri/tauri.conf.json` and in `[workspace.package]` in `Cargo.toml`.
2. Commit the change on `main`.
3. Tag the commit and push the tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The workflow in `.github/workflows/release.yml` checks that the tag matches both versions. It builds each platform into a draft release. After every build succeeds, it publishes the release with these stable file names:

| File                      | Platform                       |
| ------------------------- | ------------------------------ |
| `Noura-macOS.dmg`         | macOS, Apple silicon and Intel |
| `Noura-Windows-Setup.exe` | Windows 10 and 11              |
| `Noura-Linux.AppImage`    | Linux x86_64                   |
| `Noura-Linux.deb`         | Debian and Ubuntu              |

The website links to `releases/latest/download/<file>`, so its buttons serve the newest release without a site change.

## How the app updates

Noura checks `latest.json` on the latest GitHub release 5 seconds after launch and every 6 hours. When it finds a newer version, it downloads the bundle and verifies its signature in the background. It then offers a **Restart** button. Before installing, Noura writes any open drafts to disk. If that write fails, Noura keeps the current version running.
