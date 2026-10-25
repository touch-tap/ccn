#!/bin/sh
# Builds the npm packages for a release from its assets, ready for `npm publish` in each folder:
#   scripts/npm-packages.sh 0.2.0 dist     # dist holds the release's .tar.gz/.zip files
# Output in npm/out/: one package per platform plus @touchtap/ccn pinned to the same version.
set -eu
version=$1
dist=$2
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/npm/out"
rm -rf "$out"
mkdir -p "$out"

# npm platform-arch → Rust target
for pair in darwin-arm64:aarch64-apple-darwin darwin-x64:x86_64-apple-darwin \
            linux-arm64:aarch64-unknown-linux-musl linux-x64:x86_64-unknown-linux-musl \
            win32-arm64:aarch64-pc-windows-msvc win32-x64:x86_64-pc-windows-msvc; do
  npm_name=${pair%%:*}
  target=${pair#*:}
  os=${npm_name%-*}
  cpu=${npm_name#*-}
  dir="$out/ccn-$npm_name"
  mkdir -p "$dir/bin"
  if [ "$os" = win32 ]; then
    unzip -q -o "$dist/ccn-$target.zip" -d "$dir/bin"
  else
    tar xzf "$dist/ccn-$target.tar.gz" -C "$dir/bin"
    chmod 755 "$dir/bin/ccn"
  fi
  cat > "$dir/package.json" <<JSON
{
  "name": "@touchtap/ccn-$npm_name",
  "version": "$version",
  "description": "The ccn binary for $npm_name. Install @touchtap/ccn instead.",
  "homepage": "https://claudecodenotifier.com/cli",
  "os": ["$os"],
  "cpu": ["$cpu"],
  "files": ["bin"]
}
JSON
done

cp -R "$root/npm/ccn" "$out/ccn"
node -e '
  const fs = require("fs"), file = process.argv[1], v = process.argv[2];
  const p = JSON.parse(fs.readFileSync(file));
  p.version = v;
  for (const k of Object.keys(p.optionalDependencies)) p.optionalDependencies[k] = v;
  fs.writeFileSync(file, JSON.stringify(p, null, 2) + "\n");
' "$out/ccn/package.json" "$version"
echo "Packages in $out. Publish the platform packages first, then ccn: (cd $out/<pkg> && npm publish --access public)"
