# How do I publish a Noura release?

Push a version tag from `main`. The release workflow checks that CI passed on that commit, waits for a maintainer to approve signing, and builds signed packages with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager): a `.dmg` for macOS, an `.msi` for Windows, and a `.deb` and an AppImage for Linux. Installed copies of Noura then find the new version and update themselves. This guide covers the one-time setup, each release, local builds, and how anyone can verify a download.

## Set up the release environment once

The workflow's `build` job runs in a GitHub environment named `release`, and only that environment should hold the signing keys. Set it up in the repository settings:

1. Open **Settings** > **Environments** and create an environment named `release`.
2. Under **Deployment protection rules**, turn on **Required reviewers** and add the maintainers who approve releases. If you're the only maintainer, leave **Prevent self-review** off so you can approve your own release.
3. Under **Deployment branches and tags**, choose **Selected branches and tags** and add the tag rule `v*`.
4. Add the updater key and, for macOS, the Apple secrets described below.

If a workflow references an environment that doesn't exist, GitHub creates it with no protection rules. Until you finish these steps, a release signs without approval.

### Add the updater key

The updater installs only bundles signed with Noura's updater key, a [minisign](https://jedisct1.github.io/minisign/) key pair. Its public key is `RELEASE_PUBLIC_KEY` in `apps/desktop/src/updates.rs`. Keep the private key outside the repository, and back it up. If you lose it, installed copies can't accept future updates, and people must reinstall by hand.

The public key is the one the Tauri app used, so the private key you already have still works. Store it under the names the workflow reads, then remove the old secret:

```bash
gh secret set NOURA_UPDATER_PRIVATE_KEY --env release < ~/.tauri/noura-updater.key
gh secret set NOURA_UPDATER_PRIVATE_KEY_PASSWORD --env release
gh secret delete TAURI_SIGNING_PRIVATE_KEY --env release
```

The second command asks for the key's password. Skip it if the key has none. To start over with a new key, run `cargo packager signer generate --path ~/.noura/updater.key`, put the contents of `updater.key.pub` in `RELEASE_PUBLIC_KEY`, and store the new private key as above. Copies that shipped with the old public key won't install updates signed with the new one.

### Sign the macOS app with a Developer ID

To sign and notarize the macOS app with an Apple Developer ID, add these secrets to the `release` environment: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID`. cargo-packager imports the certificate, signs the app with the hardened runtime, and notarizes and staples it. The workflow then notarizes and staples the `.dmg`.

Without these secrets, the workflow signs the app ad hoc, and macOS asks each person to approve it in **System Settings** > **Privacy & Security** on first launch. If you set some of the six secrets but not all, the build stops and names the missing ones.

### Sign the Windows installer

The updater key signs update bundles, not installers. Windows shows a SmartScreen warning for `Noura-Windows.msi` until you sign it with an Authenticode code signing certificate. cargo-packager's `windows` settings accept a certificate thumbprint or a signing command.

## Publish a version

1. Set the new version in `[workspace.package]` in `Cargo.toml`.
2. Merge the change into `main`.
3. Tag the merged commit on `main` and push the tag:

   ```bash
   git tag v0.3.0 && git push origin v0.3.0
   ```

4. Approve the `release` deployment when GitHub asks. The workflow run page shows a **Review deployments** button.

The workflow in `.github/workflows/release.yml` runs these jobs in order:

- **verify**: fails unless the tag points to a commit on `main` and equals `v` plus the workspace version. It then waits up to 60 minutes for the CI run on that commit and fails unless CI passed.
- **create-release**: creates a draft release for the tag.
- **build**: waits for approval, then runs `scripts/package-desktop.sh` on each platform with the signing keys and uploads the packages, their updater signatures, and the license list to the draft.
- **publish**: writes the update feed `latest.json` with `scripts/write-update-feed.sh`, records build provenance for every file, adds the stable file names below, and publishes the release.

| File                   | Platform                       |
| ---------------------- | ------------------------------ |
| `Noura-macOS.dmg`      | macOS, Apple silicon and Intel |
| `Noura-Windows.msi`    | Windows 10 and 11              |
| `Noura-Linux.AppImage` | Linux x86_64                   |
| `Noura-Linux.deb`      | Debian and Ubuntu              |

The website links to `releases/latest/download/<file>`, so its buttons serve the newest release without a site change. The Tauri app shipped `Noura-Windows-Setup.exe` instead of the `.msi`. When you publish the first cargo-packager release, change the website's Windows link to `Noura-Windows.msi` in `apps/website/src/lib/site-config.ts`.

## Build the packages on your computer

Install the packaging tools once:

```bash
cargo install cargo-packager --version 0.11.8 --locked
cargo install cargo-about --version 0.9.2 --locked
```

Builds for Intel and AMD processors, including the Intel half of `--universal`, also need [NASM](https://nasm.us) 2.14 or newer on your `PATH`. rav1d, which decodes AVIF images, assembles its x86 code with it. Install it with `brew install nasm`, `sudo apt-get install nasm` or `choco install nasm`.

Then run `scripts/package-desktop.sh` from the repository root. It writes the license list, builds the app in release mode, and packages it for your operating system. The packages land in `target/release`. On macOS, `--universal` builds one app for Apple silicon and Intel instead, which needs the `aarch64-apple-darwin` and `x86_64-apple-darwin` Rust targets.

To check the result, run `scripts/check-desktop-packages.sh`. It installs or mounts each package and launches the installed app with `--smoke`, which opens the first window, prints a short report, and quits. On Linux, run it under `xvfb-run` when you have no display.

Set `CARGO_PACKAGER_SIGN_PRIVATE_KEY` to a key file or key, and `CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD` to its password, to sign each package for the updater. cargo-packager then writes a `.sig` file beside each package.

## Verify a download

Each published file has a signed build provenance attestation that links it to the workflow run and commit that built it. Check a file with the GitHub CLI:

```bash
gh attestation verify Noura-macOS.dmg -R lobbystack/noura
```

## Third-party licenses in the app

Every package ships `THIRD_PARTY_LICENSES.html`, which lists the crates the desktop binary links and their license texts. `scripts/package-desktop.sh` writes it with [cargo-about](https://github.com/EmbarkStudios/cargo-about) for the platform it builds, from `apps/desktop/packaging/about.toml` and the template `about.hbs`. cargo-about leaves out build and test dependencies, and it fails when a crate needs a license that `about.toml` doesn't accept. Keep that list in step with `deny.toml`.

cargo-packager puts the file in each package's resources: `Noura.app/Contents/Resources` on macOS, the install folder on Windows, and `/usr/lib/noura` in the `.deb` and the AppImage. The release also attaches it as `third-party-licenses-<platform>.html`.

The **Third-party licenses** link in **Settings** > **About Noura** opens this file. A build that cargo-packager didn't package has no copy, so the link opens `THIRD_PARTY_NOTICES.md` on GitHub instead.

## How the app updates

Noura checks `latest.json` on the latest GitHub release 5 seconds after launch and every 6 hours. **Settings** > **Updates** also has a **Check for updates** button. When Noura finds a newer version, it downloads the bundle and verifies its signature in the background, then offers a **Restart** button. Noura installs nothing until you choose it.

Before installing, Noura saves changes it holds back, such as typing that waits for a pause. It then waits for every pending write to reach disk, including a workspace you're still opening or creating. If a save or a write fails, Noura keeps running the current version and says why, naming the file when it knows it.

The install can take a while, for example while macOS asks for an administrator password, and you can keep working while it runs. After the install, Noura saves and waits again before it restarts. If that second round fails, Noura keeps running, and the new version starts the next time you open Noura.

When you quit Noura, it waits up to 30 seconds after its windows close for writes still running. A restart for an update quits the same way, and every open window comes back in the new version.

On macOS the update replaces the app bundle. On Windows the `.msi` installer ends the running app as soon as it starts, so Noura keeps the update until its windows have closed and its writes have finished. Noura then starts the installer, which opens the new version when it's done.

On Linux, only the AppImage updates itself. The `.deb` package runs without the updater, so people who use it install each new version from the website. Development builds never update themselves.

## Release checks in CI

The `packages` job in `.github/workflows/ci.yml` runs on every push and pull request. On macOS, Windows, and Ubuntu 22.04, it builds each package, signs it with a throwaway updater key, and runs `scripts/check-desktop-packages.sh`. The check fails unless each installed app draws its first window and finds its license list. It also checks that the `.dmg`, the `.msi` and the AppImage update themselves and the `.deb` doesn't.

The desktop crate's tests run the real updater against a feed served on a local port. They check that it accepts a bundle signed with the expected key and rejects a changed bundle or another key's signature.

## Known limitation: signing runs in the build job

The `build` job compiles the app in the same step that holds the updater key. The key reaches only that step, but a dependency's build script that runs during it could read the key. A stricter setup would build with a throwaway key in one job, then sign the update bundles and write `latest.json` in a separate job that compiles nothing. Until then, the required reviewer on the `release` environment and the `--locked` builds limit the exposure.
