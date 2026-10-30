#!/bin/sh
# Updates the Scoop manifest for a published release, in this repo and in a checkout of the bucket:
#   scripts/scoop-bucket.sh 0.2.0                 # downloads the release's SHA256SUMS from GitHub
#   scripts/scoop-bucket.sh 0.2.0 dist/SHA256SUMS # or uses a local copy
# Writes packaging/scoop/ccn.json here, then bucket/ccn.json in the touch-tap/scoop-bucket checkout
# (SCOOP_BUCKET_DIR, default ../scoop-bucket next to this repo) and commits there. Push it yourself.
# The owner creates the bucket repository once; see RELEASING.md.
set -eu
version=$1
sums=${2:-}
tag="v$version"
root="$(cd "$(dirname "$0")/.." && pwd)"
bucket="${SCOOP_BUCKET_DIR:-$root/../scoop-bucket}"
base="https://github.com/touch-tap/ccn/releases/download/$tag"

fail() { echo "scoop-bucket: $*" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [ -z "$sums" ]; then
  sums="$work/SHA256SUMS"
  curl -fsSL "$base/SHA256SUMS" -o "$sums" || fail "no SHA256SUMS at $base (is $tag published?)"
fi
[ -f "$sums" ] || fail "$sums isn't a file"

sha() {
  s=$(awk -v f="ccn-$1-pc-windows-msvc.zip" '$2 == f || $2 == "*" f { print tolower($1) }' "$sums")
  [ -n "$s" ] || fail "no ccn-$1-pc-windows-msvc.zip in $sums"
  printf '%s' "$s"
}
x64=$(sha x86_64)
arm64=$(sha aarch64)

# `checkver: github` watches the releases feed; `autoupdate` fills the next version in from the
# same asset names and reads each hash from that release's SHA256SUMS, so the bucket's own
# `checkver -u` run (or Scoop's Excavator) can bump it without this script.
manifest() {
cat <<JSON
{
    "version": "$version",
    "description": "Claude Code Notifier for the terminal: phone notifications and approvals for Claude Code",
    "homepage": "https://claudecodenotifier.com/cli",
    "license": {
        "identifier": "PolyForm-Shield-1.0.0",
        "url": "https://github.com/touch-tap/ccn/blob/main/LICENSE.md"
    },
    "suggest": {
        "vcredist": "extras/vcredist2022"
    },
    "architecture": {
        "64bit": {
            "url": "$base/ccn-x86_64-pc-windows-msvc.zip",
            "hash": "$x64"
        },
        "arm64": {
            "url": "$base/ccn-aarch64-pc-windows-msvc.zip",
            "hash": "$arm64"
        }
    },
    "bin": "ccn.exe",
    "checkver": "github",
    "autoupdate": {
        "architecture": {
            "64bit": {
                "url": "https://github.com/touch-tap/ccn/releases/download/v\$version/ccn-x86_64-pc-windows-msvc.zip",
                "hash": {
                    "url": "\$baseurl/SHA256SUMS"
                }
            },
            "arm64": {
                "url": "https://github.com/touch-tap/ccn/releases/download/v\$version/ccn-aarch64-pc-windows-msvc.zip",
                "hash": {
                    "url": "\$baseurl/SHA256SUMS"
                }
            }
        }
    },
    "notes": [
        "Connect your phone: run",
        "  ccn",
        "and scan the QR code with the phone's camera, or run",
        "  ccn pair <the code on your phone>"
    ]
}
JSON
}

manifest > "$root/packaging/scoop/ccn.json"
echo "Wrote packaging/scoop/ccn.json"

[ -d "$bucket/.git" ] || fail "no checkout of touch-tap/scoop-bucket at $bucket (set SCOOP_BUCKET_DIR, or see RELEASING.md)"
mkdir -p "$bucket/bucket"
cp "$root/packaging/scoop/ccn.json" "$bucket/bucket/ccn.json"
if git -C "$bucket" diff --quiet -- bucket/ccn.json && [ -z "$(git -C "$bucket" ls-files --others --exclude-standard bucket/ccn.json)" ]; then
  echo "$bucket/bucket/ccn.json already describes $version"
  exit 0
fi
git -C "$bucket" add bucket/ccn.json
git -C "$bucket" commit -q -m "ccn $version" -- bucket/ccn.json
echo "Committed ccn $version in $bucket; push it with: git -C $bucket push"
