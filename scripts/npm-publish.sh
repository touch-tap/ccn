#!/bin/sh
# Publishes the packages scripts/npm-packages.sh built, platform packages first, skipping any version
# already on npm, so a run that outlives its one-time password can simply be repeated:
#   scripts/npm-publish.sh 123456
set -eu
otp=${1:?usage: scripts/npm-publish.sh <one-time password>}
out="$(cd "$(dirname "$0")/.." && pwd)/npm/out"
for dir in "$out"/ccn-* "$out/ccn"; do
  name=$(node -p "require('$dir/package.json').name")
  version=$(node -p "require('$dir/package.json').version")
  if npm view "$name@$version" version >/dev/null 2>&1; then
    echo "$name@$version is already published"
  else
    (cd "$dir" && npm publish --access public --otp "$otp")
  fi
done
