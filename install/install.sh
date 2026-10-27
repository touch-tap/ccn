#!/bin/sh
# Installs ccn, the Claude Code Notifier CLI, on macOS or Linux.
#
#   curl -fsSL https://claudecodenotifier.com/install.sh | sh              # install only
#   curl -fsSL https://claudecodenotifier.com/install.sh | sh -s 482913    # install and pair
#   curl -fsSL https://claudecodenotifier.com/install.sh | sh -s 482913 --licence CCN-XXXX-XXXX-XXXX-XXXX
#   curl -fsSL https://claudecodenotifier.com/install.sh | sh -s -- --licence CCN-XXXX-XXXX-XXXX-XXXX
#                                                     # install, use a licence key (and pair)
#
# CCN_VERSION=0.2.0 picks a release (default: the latest). CCN_INSTALL_DIR overrides ~/.local/bin.
# CCN_NO_ANALYTICS=1 (or DO_NOT_TRACK=1) stops the anonymous install events.
set -eu

repo="touch-tap/ccn"
code=""
licence=""
while [ $# -gt 0 ]; do
  case "$1" in
    --licence|--license) licence="${2:-}"; [ $# -ge 2 ] && shift; shift ;;
    --licence=*|--license=*) licence="${1#*=}"; shift ;;
    --) shift ;;
    *) [ -z "$code" ] && code="$1"; shift ;;
  esac
done
dir="${CCN_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { track cli_install_failed "\"stage\":\"$stage\""; printf 'ccn install: %s\n' "$*" >&2; exit 1; }

# One anonymous event per step, so we can see where installs fail: the event name, the stage, the
# OS and processor. Nothing else. CCN_NO_ANALYTICS=1 or DO_NOT_TRACK=1 turns it off.
stage="start"
case "$(uname -s)" in Darwin) model="macos" ;; Linux) model="linux" ;; *) model="other" ;; esac
case "$(uname -m)" in arm64|aarch64) model="$model-aarch64" ;; x86_64|amd64) model="$model-x86_64" ;; *) model="$model-other" ;; esac
track() {
  case "${CCN_NO_ANALYTICS:-}${DO_NOT_TRACK:-}" in ""|0|00) ;; *) return 0 ;; esac
  body="{\"context\":{\"platform\":\"cli\",\"deviceModel\":\"$model\",\"environment\":\"production\"},\"events\":[{\"name\":\"$1\",\"properties\":{\"method\":\"script\"${2:+,$2}}}]}"
  url="${CCN_SERVER_URL:-https://ccnotifier-api.touchtap.workers.dev}/api/events"
  if command -v curl >/dev/null 2>&1; then
    curl -fsS -m 3 -H 'Content-Type: application/json' -d "$body" "$url" >/dev/null 2>&1 || true
  elif command -v wget >/dev/null 2>&1; then
    wget -q -T 3 -O /dev/null --header='Content-Type: application/json' --post-data="$body" "$url" >/dev/null 2>&1 || true
  fi
}
track cli_install_started "\"pairing\":\"$([ -n "$code" ] && echo yes || echo no)\""

stage="unsupported"
case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-musl" ;;
  *) fail "this script is for macOS and Linux. On Windows run: irm https://claudecodenotifier.com/install.ps1 | iex" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64) arch="x86_64" ;;
  *) fail "unsupported processor $(uname -m)" ;;
esac
# A shell running under Rosetta reports x86_64; the native build is better
if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
  arch="aarch64"
fi
asset="ccn-$arch-$os.tar.gz"

if [ -n "${CCN_DOWNLOAD_BASE:-}" ]; then
  base="$CCN_DOWNLOAD_BASE"   # for testing the script against a local build
elif [ -n "${CCN_VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/v${CCN_VERSION#v}"
else
  base="https://github.com/$repo/releases/latest/download"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
else
  fail "needs curl or wget"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

stage="download"
say "Downloading ccn for $arch-$os..."
fetch "$base/$asset" "$tmp/$asset" || fail "could not download $base/$asset"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || fail "could not download the checksums"

stage="checksum"
expected="$(awk -v f="$asset" '$2 == f { print $1 }' "$tmp/SHA256SUMS")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | awk '{ print $1 }')"
else
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')"
fi
[ -n "$expected" ] && [ "$expected" = "$actual" ] || fail "checksum mismatch for $asset"

stage="unpack"
tar xzf "$tmp/$asset" -C "$tmp" || fail "could not unpack $asset"
mkdir -p "$dir"
# Write beside the target and rename, so a hook running the old binary is never half-overwritten
cp "$tmp/ccn" "$dir/.ccn.new"
chmod 755 "$dir/.ccn.new"
mv "$dir/.ccn.new" "$dir/ccn"
stage="run"
version="$("$dir/ccn" --version)" || fail "the downloaded ccn did not run"
say "Installed $version to $dir/ccn"

ccn="ccn"
case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    ccn="$dir/ccn"
    say ""
    say "$dir is not on your PATH. The hooks don't need it, but to type ccn yourself add this to your shell profile:"
    say "  export PATH=\"$dir:\$PATH\""
    ;;
esac

on_path=yes
[ "$ccn" = ccn ] || on_path=no
track cli_install_finished "\"pairing\":\"$([ -n "$code" ] && echo yes || echo no)\",\"on_path\":\"$on_path\""

export CCN_INSTALL_METHOD=script
# The licence first, so the phone paired next is unlocked as soon as it connects. A bad key doesn't
# stop the install; the message says what to do.
if [ -n "$licence" ]; then
  say ""
  "$dir/ccn" activate "$licence" < /dev/null || say "The licence wasn't applied. Try again later: $ccn activate $licence"
fi
if [ -n "$code" ]; then
  say ""
  # stdin is the piped script, so give pair the terminal for any question it asks
  if (: < /dev/tty) 2>/dev/null; then
    exec "$dir/ccn" pair "$code" < /dev/tty
  else
    exec "$dir/ccn" pair "$code"
  fi
elif (: < /dev/tty) 2>/dev/null; then
  # No code: show a QR code to scan with the phone, which also offers the app if it isn't installed
  say ""
  exec "$dir/ccn" pair < /dev/tty
else
  say ""
  say "Next: run \`$ccn\` and scan the QR code with your phone, or open Claude Code Notifier,"
  say "add a computer, and run"
  say "  $ccn pair <the code on your phone>"
fi
