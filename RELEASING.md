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
