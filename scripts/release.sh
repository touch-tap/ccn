#!/bin/sh
# Starts a release from this Mac, which holds the Developer ID certificate:
#   scripts/release.sh 0.2.0
#   scripts/release.sh 0.2.0 --resume   (after the wait failed: skips to signing the macOS binaries)
# Builds the macOS notifier app, signs and notarizes it, creates the DRAFT release with it attached,
# then pushes the tag. The Release workflow builds every target with that app embedded and adds them
# to the draft; this script waits for it, then signs and notarizes the two macOS binaries with the
# Developer ID and replaces them (and SHA256SUMS and ccn.rb) in the draft. Needs the certificate in the keychain and the `ccn-notary` notarytool profile
# (see RELEASING.md).
set -eu
version=$1
resume=${2:-}
tag="v$version"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

fail() { echo "release: $*" >&2; exit 1; }

[ "$(git branch --show-current)" = main ] || fail "not on main"
[ -z "$(git status --porcelain)" ] || fail "the working tree has changes"
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || fail "main isn't the same as origin/main"
grep -q "^version = \"$version\"$" Cargo.toml || fail "Cargo.toml's version isn't $version"
if [ "$resume" = --resume ]; then
  git fetch -q origin tag "$tag"
  sha=$(git rev-parse "$tag^{commit}")
  gh release view "$tag" --json isDraft --jq .isDraft | grep -q true || fail "$tag isn't a draft release"
else
  ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null || fail "$tag already exists"
  sha=$(git rev-parse HEAD)
fi

identity=${CODESIGN_IDENTITY:-$(security find-identity -v -p codesigning \
  | grep -o '"Developer ID Application[^"]*"' | head -1 | tr -d '"')}
[ -n "$identity" ] || fail "no Developer ID Application certificate in the keychain"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [ "$resume" != --resume ]; then
CODESIGN_IDENTITY="$identity" scripts/build-macos-notifier.sh "$version" "$work" >/dev/null
app="$work/Claude Code Notifier.app"

echo "Notarizing the notifier app (a few minutes)..."
ditto -c -k --keepParent "$app" "$work/submit.zip"
xcrun notarytool submit "$work/submit.zip" --keychain-profile ccn-notary --wait
xcrun stapler staple "$app"
spctl --assess --type exec "$app" || fail "Gatekeeper rejects the notarized app"

ditto -c -k --keepParent "$app" "$work/ccn-notifier-macos.zip"
gh release create "$tag" "$work/ccn-notifier-macos.zip" --draft --target "$sha" \
  --title "ccn $tag" --generate-notes
git tag "$tag"
git push origin "$tag"
fi

# The workflow builds every target. The macOS binaries then get the Developer ID signature here,
# where the certificate is: with a stable identity, the keychain lets each new version read the
# token the last one stored, instead of prompting in the middle of a hook.
echo "Waiting for the Release workflow..."
run_id=""
for _ in $(seq 1 30); do
  # Matched on the commit too: a tag deleted and pushed again leaves the old tag's runs listed
  run_id=$(gh run list --workflow release.yml --event push --limit 10 --json databaseId,headBranch,headSha \
    --jq ".[] | select(.headBranch == \"$tag\" and .headSha == \"$sha\") | .databaseId" | head -1)
  [ -n "$run_id" ] && break
  sleep 5
done
[ -n "$run_id" ] || fail "the Release workflow for $tag didn't start"
gh run watch "$run_id" --exit-status >/dev/null || fail "the Release workflow failed: gh run view $run_id"

echo "Signing and notarizing the macOS binaries..."
mac="$work/mac"
mkdir -p "$mac/submit"
gh release download "$tag" -D "$mac" -p 'ccn-*-apple-darwin.tar.gz' -p SHA256SUMS
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  mkdir -p "$mac/$target"
  tar xzf "$mac/ccn-$target.tar.gz" -C "$mac/$target"
  codesign --force --options runtime --timestamp --identifier com.touchtap.ccn --sign "$identity" "$mac/$target/ccn"
  cp "$mac/$target/ccn" "$mac/submit/ccn-$target"
done
ditto -c -k "$mac/submit" "$mac/submit.zip"
xcrun notarytool submit "$mac/submit.zip" --keychain-profile ccn-notary --wait
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  codesign -dv "$mac/$target/ccn" 2>&1 | grep -q "TeamIdentifier=4ABJZNGY79" || fail "$target isn't signed by TouchTap"
  tar czf "$mac/ccn-$target.tar.gz" -C "$mac/$target" ccn
  sum=$(shasum -a 256 "$mac/ccn-$target.tar.gz" | cut -d' ' -f1)
  awk -v f="ccn-$target.tar.gz" -v s="$sum" '$2 == f { print s "  " f; next } { print }' "$mac/SHA256SUMS" > "$mac/SHA256SUMS.new"
  mv "$mac/SHA256SUMS.new" "$mac/SHA256SUMS"
done
scripts/brew-formula.sh "$version" "$mac/SHA256SUMS" > "$mac/ccn.rb"
gh release upload "$tag" --clobber "$mac/ccn-aarch64-apple-darwin.tar.gz" "$mac/ccn-x86_64-apple-darwin.tar.gz" \
  "$mac/SHA256SUMS" "$mac/ccn.rb"
echo "Done: $tag is a draft with signed macOS binaries. Check it, then publish it."
