# Releasing ccn

1. Bump `version` in `Cargo.toml` (and run `cargo build` so `Cargo.lock` follows), commit.
2. `git tag v0.2.0 && git push origin v0.2.0`. The Release workflow builds six targets and creates a
   **draft** GitHub release with the archives, `SHA256SUMS` and `ccn.rb`.
3. Check the draft, then publish it. The install scripts download from
   `releases/latest/download/…`, so they serve the new version the moment it is published.
   The repository must be public for the scripts, Homebrew and npm to reach the assets.
4. Homebrew: copy `ccn.rb` from the release to `touch-tap/homebrew-tap` as `Formula/ccn.rb`, push.
5. npm: download the release assets into `dist/`, run `scripts/npm-packages.sh 0.2.0 dist`, then
   `npm publish --access public` in each `npm/out/ccn-*` folder and finally in `npm/out/ccn`.
6. Website: `install/install.sh` and `install/install.ps1` are served from claudecodenotifier.com.
   Copy them to the CCNotifier_Website repo root when they change.

The phone apps read the install lines from `GET /api/cli/install` on the server
(`CCNotifier_Server/src/routes/install.ts`), so a change of install method needs no app release.

## The macOS notifier app

macOS notifications come from "Claude Code Notifier.app" (`macos/notifier`), built by
`scripts/build-macos-notifier.sh` and embedded in the macOS binaries (`build.rs`, via
`CCN_NOTIFIER_APP`). ccn writes it to its config folder and `ccn setup` asks for permission. A build
without it falls back to AppleScript notifications, which show Script Editor's icon.

Its icon is the phone apps' `AppIcon.icon` copied from the iOS repo; copy it again when that changes.

The Release workflow signs and notarizes it when these repository secrets exist, and ad-hoc signs it
otherwise (that works, but isn't what to ship):

- `DEVELOPER_ID_P12`: a Developer ID Application certificate with its private key, exported as .p12
  and base64-encoded (`base64 -i cert.p12 | pbcopy`)
- `DEVELOPER_ID_P12_PASSWORD`: the .p12's export password
- `NOTARY_KEY`, `NOTARY_KEY_ID`, `NOTARY_ISSUER`: an App Store Connect API key (the .p8 file's
  contents, its key ID and the issuer ID) with the Developer role, for `notarytool`
