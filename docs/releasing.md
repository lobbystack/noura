# How do I publish a noura release?

Push a version tag from `main`. The release workflow checks that CI passed on that commit, waits for a maintainer to approve signing, and builds signed installers for macOS, Windows, and Linux. Installed copies of noura then find the new version and update themselves. This guide covers the one-time setup, each release, and how anyone can verify a download.

## Set up the release environment once

The updater only installs bundles signed with the noura updater key. The public key lives in `src-tauri/tauri.conf.json`. Keep the private key outside the repository, and back it up. If you lose it, installed apps can't accept future updates, and users must reinstall by hand.

The workflow's `build` job runs in a GitHub environment named `release`, and only that environment should hold the private key. Set it up in the repository settings:

1. Open **Settings** > **Environments** and create an environment named `release`.
2. Under **Deployment protection rules**, turn on **Required reviewers** and add the maintainers who approve releases. If you're the only maintainer, leave **Prevent self-review** off so you can approve your own release.
3. Under **Deployment branches and tags**, choose **Selected branches and tags** and add the tag rule `v*`.
4. Add the signing key as an environment secret, then remove the repository-level copy so other workflows can't read it:

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY --env release < ~/.tauri/noura-updater.key
gh secret delete TAURI_SIGNING_PRIVATE_KEY
```

If you protected the key with a password, set `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the `release` environment the same way and delete its repository-level copy.

If a workflow references an environment that doesn't exist, GitHub creates it with no protection rules. Until you finish these steps, a release signs without approval, using the repository-level secret.

### Sign the macOS app with a Developer ID

To sign and notarize the macOS app with an Apple Developer ID, add these secrets to the `release` environment: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`. Without them, the workflow signs the app ad hoc, and macOS asks each user to approve it in **System Settings** > **Privacy & Security** on first launch.

### Sign the Windows installer

The updater key signs update bundles, not installers. Windows shows a SmartScreen warning for `Noura-Windows-Setup.exe` until you sign it with an Authenticode code signing certificate. Tauri documents the setup in [Windows code signing](https://v2.tauri.app/distribute/sign/windows/).

## Publish a version

1. Set the same version in `src-tauri/tauri.conf.json` and in `[workspace.package]` in `Cargo.toml`.
2. Merge the change into `main`.
3. Tag the merged commit on `main` and push the tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

4. Approve the `release` deployment when GitHub asks. The workflow run page shows a **Review deployments** button.

The workflow in `.github/workflows/release.yml` runs these jobs in order:

- **verify**: fails unless the tag points to a commit on `main` and matches both version fields. It then waits up to 60 minutes for the CI run on that commit and fails unless CI passed.
- **create-release**: creates a draft release for the tag.
- **build**: waits for approval, then builds and signs each platform and uploads it to the draft.
- **publish**: records build provenance for every file, adds the stable file names below, and publishes the release.

| File                      | Platform                       |
| ------------------------- | ------------------------------ |
| `Noura-macOS.dmg`         | macOS, Apple silicon and Intel |
| `Noura-Windows-Setup.exe` | Windows 10 and 11              |
| `Noura-Linux.AppImage`    | Linux x86_64                   |
| `Noura-Linux.deb`         | Debian and Ubuntu              |

The website links to `releases/latest/download/<file>`, so its buttons serve the newest release without a site change.

## Verify a download

Each published file has a signed build provenance attestation that links it to the workflow run and commit that built it. Check a file with the GitHub CLI:

```bash
gh attestation verify Noura-macOS.dmg -R lobbystack/noura
```

## Third-party licenses in the app

Before each platform build, the workflow builds the frontend and runs `scripts/generate-third-party-licenses.ts`. The script lists the npm packages in the desktop frontend bundle and the crates that `noura-desktop` links, with their license texts. `src-tauri/tauri.release.conf.json` bundles the result as `THIRD_PARTY_LICENSES.txt`, and the workflow also attaches it to the release as `third-party-licenses-<platform>.txt`. CI runs the same script to catch breakage before a release.

To generate the file locally:

```bash
bun run --cwd apps/app build
bun scripts/generate-third-party-licenses.ts --out src-tauri/THIRD_PARTY_LICENSES.txt
```

## How the app updates

noura checks `latest.json` on the latest GitHub release 5 seconds after launch and every 6 hours. When it finds a newer version, it downloads the bundle and verifies its signature in the background. It then offers a **Restart** button. Before installing, noura writes any open drafts to disk. If that write fails, noura keeps the current version running.

On Linux, only the AppImage updates itself. The `.deb` package runs without the updater, so its users install each new version from the website.

## Known limitation: signing runs in the build job

The `build` job installs JavaScript dependencies and runs the frontend build in the same job that holds the signing key. The key reaches only the Tauri build step, but code from any dependency that runs during that step could read it. A stricter setup would build with a throwaway key in one job, then re-sign the updater bundles and rewrite `latest.json` in a separate job that installs nothing. Until then, the required reviewer on the `release` environment and the `--frozen-lockfile` install limit the exposure.
