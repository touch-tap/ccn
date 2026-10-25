#!/bin/sh
# Publishes the packages scripts/npm-packages.sh built, platform packages first, skipping any version
# already on npm, so an interrupted run can simply be repeated:
#   scripts/npm-publish.sh            # in a real terminal: npm asks for your passkey in the browser
#   scripts/npm-publish.sh 123456     # with an authenticator app's one-time password
#   NPM_CONFIG_USERCONFIG=… scripts/npm-publish.sh   # with a granular token that bypasses 2FA
set -eu
otp=${1:-}
out="$(cd "$(dirname "$0")/.." && pwd)/npm/out"
for dir in "$out"/ccn-* "$out/ccn"; do
  name=$(node -p "require('$dir/package.json').name")
  version=$(node -p "require('$dir/package.json').version")
  if npm view "$name@$version" version >/dev/null 2>&1; then
    echo "$name@$version is already published"
  else
    (cd "$dir" && npm publish --access public --auth-type=web ${otp:+--otp "$otp"})
  fi
done
