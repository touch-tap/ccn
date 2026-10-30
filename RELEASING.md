# Releasing ccn

1. Bump `version` in `Cargo.toml` (and run `cargo build` so `Cargo.lock` follows), commit.
2. On the release Mac, `scripts/release.sh 0.2.0`. It signs and notarizes the macOS notifier app,
   creates a **draft** GitHub release with it, and pushes the tag. The Release workflow then builds
   six targets and adds the archives, `SHA256SUMS` and `ccn.rb` to that draft. The script waits for
   it, then signs the two macOS binaries with the Developer ID (identifier `com.touchtap.ccn`,
   hardened runtime), notarizes them, and replaces them, `SHA256SUMS` and `ccn.rb` in the draft.
   The stable signature is what lets each version read the keychain token without a prompt. Never
   push a release tag by hand: the macOS builds need the notarized app from the draft and fail
   without it, and the binaries would stay unsigned.
3. Check the draft, then publish it. The install scripts download from
   `releases/latest/download/…`, so they serve the new version the moment it is published.
   The repository must be public for the scripts, Homebrew and npm to reach the assets.
4. Homebrew: copy `ccn.rb` from the release to `touch-tap/homebrew-tap` as `Formula/ccn.rb`, push.
5. winget: `scripts/winget-manifests.sh 0.2.0` downloads the release's `SHA256SUMS` and writes the
   three manifests to `packaging/winget/manifests/t/TouchTap/ccn/0.2.0/` (schema 1.28.0; commit
   them). Submit them to [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) either
   with `wingetcreate submit --token <github token> packaging/winget/manifests/t/TouchTap/ccn/0.2.0`
   on a Windows machine, or by hand: fork `winget-pkgs`, copy the folder to the same path, and open
   a PR titled `New version: TouchTap.ccn version 0.2.0` (`New package:` the first time). The
   first submission is reviewed by a person; later ones are usually merged by the bot once its
   validation passes, a day or two later. `winget install TouchTap.ccn` works once it is merged.
6. Scoop: `scripts/scoop-bucket.sh 0.2.0` writes `packaging/scoop/ccn.json` here (commit it) and
   `bucket/ccn.json` in a checkout of `touch-tap/scoop-bucket` next to this repo (or at
   `SCOOP_BUCKET_DIR`), and commits there; push that checkout. The bucket repository is created
   once by the owner: `gh repo create touch-tap/scoop-bucket --public --clone`, then run the
   script, push, and users add it with `scoop bucket add touch-tap
   https://github.com/touch-tap/scoop-bucket`. The manifest's `checkver`/`autoupdate` fields also
   let Scoop's own tooling (`bin/checkver.ps1 -u`, if the bucket is made from
   ScoopInstaller/BucketTemplate) bump it from the next release's `SHA256SUMS` without this script.
7. npm: download the release assets into `dist/`, run `scripts/npm-packages.sh 0.2.0 dist`, then
   `scripts/npm-publish.sh` in a real terminal (signed in with `npm login` as an owner of the
   `touchtap` org; npm asks for the passkey in the browser). It skips versions already on npm, so
   an interrupted run can be repeated.
8. Website: `install/install.sh` and `install/install.ps1` are served from claudecodenotifier.com.
   Copy them to the CCNotifier_Website repo root when they change.

The phone apps read the install lines from `GET /api/cli/install` on the server
(`CCNotifier_Server/src/routes/install.ts`), so a change of install method needs no app release.

## The macOS notifier app

macOS notifications come from "Claude Code Notifier.app" (`macos/notifier`), built by
`scripts/build-macos-notifier.sh` and embedded in the macOS binaries (`build.rs`, via
`CCN_NOTIFIER_APP`). ccn writes it to its config folder and `ccn setup` asks for permission. A build
without it falls back to AppleScript notifications, which show Script Editor's icon.

Its icon is the phone apps' `AppIcon.icon` copied from the iOS repo; copy it again when that changes.

It is signed and notarized on the release Mac, never in CI, so the Developer ID private key stays
in that Mac's keychain. The Release workflow refuses an app that isn't notarized Developer ID.

### Setting up a release Mac (once per Mac)

1. Xcode (for `swiftc`, `actool`, `notarytool`), `gh` signed in to GitHub, and this repo.
2. The certificate: "Developer ID Application: TouchTap LTD (4ABJZNGY79)" with its private key in the
   login keychain. From the old Mac, export it from Keychain Access (My Certificates, right-click,
   Export, .p12) and double-click the .p12 on the new one. If the old Mac is gone, create a new
   Developer ID Application certificate at developer.apple.com (Certificates, +); releases signed
   with the old one stay valid. Check with `security find-identity -v -p codesigning`.
3. The notarization profile, using an App Store Connect API key (Users and Access, Integrations;
   the .p8 is only downloadable once, so keep it with the company keys):
   `xcrun notarytool store-credentials ccn-notary --key AuthKey_XXXX.p8 --key-id XXXX --issuer <issuer id>`
