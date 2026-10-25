#!/bin/sh
# Starts a release from this Mac, which holds the Developer ID certificate:
#   scripts/release.sh 0.2.0
# Builds the macOS notifier app, signs and notarizes it, creates the DRAFT release with it attached,
# then pushes the tag. The Release workflow builds every target with that app embedded and adds them
# to the draft. Needs the certificate in the keychain and the `ccn-notary` notarytool profile
# (see RELEASING.md).
set -eu
version=$1
tag="v$version"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

fail() { echo "release: $*" >&2; exit 1; }

[ "$(git branch --show-current)" = main ] || fail "not on main"
[ -z "$(git status --porcelain)" ] || fail "the working tree has changes"
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || fail "main isn't the same as origin/main"
grep -q "^version = \"$version\"$" Cargo.toml || fail "Cargo.toml's version isn't $version"
! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || fail "$tag already exists"

identity=${CODESIGN_IDENTITY:-$(security find-identity -v -p codesigning \
  | grep -o '"Developer ID Application[^"]*"' | head -1 | tr -d '"')}
[ -n "$identity" ] || fail "no Developer ID Application certificate in the keychain"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
CODESIGN_IDENTITY="$identity" scripts/build-macos-notifier.sh "$version" "$work" >/dev/null
app="$work/Claude Code Notifier.app"

echo "Notarizing the notifier app (a few minutes)..."
ditto -c -k --keepParent "$app" "$work/submit.zip"
xcrun notarytool submit "$work/submit.zip" --keychain-profile ccn-notary --wait
xcrun stapler staple "$app"
spctl --assess --type exec "$app" || fail "Gatekeeper rejects the notarized app"

ditto -c -k --keepParent "$app" "$work/ccn-notifier-macos.zip"
gh release create "$tag" "$work/ccn-notifier-macos.zip" --draft --target "$(git rev-parse HEAD)" \
  --title "ccn $tag" --generate-notes
git tag "$tag"
git push origin "$tag"
echo "Pushed $tag. Watch the Release workflow, then check and publish the draft."
